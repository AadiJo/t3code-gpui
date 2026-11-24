//! Command text as a work row shows it (`commandLabel.ts`): the script inside a lone shell
//! wrapper (`bash -lc '…'`, `pwsh -Command …`, `cmd /c …`) with control characters drawn as
//! Unicode control pictures, the program a command runs for "Ran pnpm" labels, and whether
//! its text highlights as PowerShell or shell script.
//!
//! Scanning works on `char`s where the TypeScript indexes UTF-16 units; the two agree for
//! every character the scanners inspect (all ASCII).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

const MAX_COMMAND_SEGMENTS: usize = 64;

const SHELL_PROGRAMS: &[&str] = &["sh", "bash", "zsh", "dash", "ash", "ksh", "fish"];
const WINDOWS_SHELL_PROGRAMS: &[&str] = &["cmd", "cmd.exe", "powershell", "powershell.exe", "pwsh", "pwsh.exe"];
const SHELL_OPTIONS_WITH_VALUE: &[&str] = &["-o", "-O", "--rcfile", "--init-file"];
const SHELL_COMMAND_WRAPPERS: &[&str] = &["builtin", "command", "exec"];
const SHELL_PRECOMMAND_MODIFIERS: &[&str] = &["nocorrect", "noglob", "time"];
const POWERSHELL_SETUP_PROGRAMS: &[&str] = &["pop-location", "push-location", "set-location"];
const POWERSHELL_FLAGS: &[&str] = &["-mta", "-nologo", "-noninteractive", "-noprofile", "-sta"];
const POWERSHELL_OPTIONS_WITH_VALUE: &[&str] =
    &["-configurationname", "-executionpolicy", "-inputformat", "-outputformat", "-version", "-windowstyle", "-workingdirectory"];
const START_PROCESS_FLAGS: &[&str] =
    &["-confirm", "-debug", "-loaduserprofile", "-nonewwindow", "-passthru", "-usenewenvironment", "-verbose", "-wait", "-whatif"];
const START_PROCESS_OPTIONS_WITH_VALUE: &[&str] = &[
    "-argumentlist",
    "-credential",
    "-environment",
    "-erroraction",
    "-errorvariable",
    "-informationaction",
    "-informationvariable",
    "-outbuffer",
    "-outvariable",
    "-pipelinevariable",
    "-progressaction",
    "-redirectstandarderror",
    "-redirectstandardinput",
    "-redirectstandardoutput",
    "-verb",
    "-warningaction",
    "-warningvariable",
    "-windowstyle",
    "-workingdirectory",
];
const SKIPPABLE_SUDO_PROBES: &[&str] = &["[", "[[", "test", "true"];
const NON_PROGRAM_PREFIX_CHARACTERS: &str = "<>(){}[];|&$`#!%@:";
const NON_PROGRAM_SUFFIX_CHARACTERS: &str = "){]}`";

const NON_DESCRIPTIVE_SHELL_PROGRAMS: &[&str] = &[
    "!", "#", ".", ":", "[", "[[", "alias", "and", "autoload", "begin", "bg", "bind", "bindkey", "break", "builtin", "caller",
    "case", "catch", "cd", "command", "compgen", "complete", "compopt", "continue", "coproc", "declare", "dirs", "disown", "do",
    "done", "elif", "else", "enable", "end", "esac", "eval", "exec", "exit", "export", "false", "fc", "fg", "fi", "finally",
    "for", "foreach", "function", "getopts", "history", "if", "in", "jobs", "let", "local", "logout", "mapfile", "nocorrect",
    "noglob", "not", "or", "popd", "pushd", "read", "readarray", "readonly", "repeat", "return", "select", "set", "setopt",
    "shift", "shopt", "source", "switch", "suspend", "test", "then", "time", "times", "trap", "try", "true", "type", "typeset",
    "ulimit", "umask", "unalias", "until", "unset", "unsetopt", "wait", "while",
];

const TERMINAL_SHELL_PROGRAMS: &[&str] = &[
    "and", "begin", "break", "case", "catch", "continue", "coproc", "do", "done", "elif", "else", "end", "esac", "eval", "exec",
    "exit", "false", "fi", "finally", "for", "foreach", "function", "if", "in", "not", "or", "repeat", "return", "select",
    "switch", "then", "try", "until", "while",
];

const POWERSHELL_VERBS: &str = "Add Approve Assert Backup Block Build Checkpoint Clear Close Compare Complete Compress Confirm Connect Convert ConvertFrom ConvertTo Copy Debug Deny Deploy Disable Disconnect Dismount Edit Enable Enter Exit Expand Export Find ForEach Format Get Grant Group Hide Import Initialize Install Invoke Join Limit Lock Measure Merge Mount Move New Open Optimize Out Ping Pop Protect Publish Push Read Receive Redo Register Remove Rename Repair Request Reset Resize Resolve Restart Restore Resume Revoke Save Search Select Send Set Show Skip Sort Split Start Step Stop Submit Suspend Switch Sync Tee Test Trace Unblock Undo Uninstall Unlock Unprotect Unpublish Unregister Update Use Wait Watch Where Write";

macro_rules! regex {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).expect("valid regex"));
    };
}

regex!(ASSIGNMENT_PREFIX, r"^[A-Za-z_][A-Za-z0-9_]*\+?=");
regex!(URL_SCHEME, r"^[A-Za-z][A-Za-z0-9+.-]*:");
regex!(LEADING_WORD, r"^\S+");
regex!(CALL_OPERATOR, r"^\s*&\s+([\s\S]*)$");
regex!(POWERSHELL_ASSIGNMENT, r"(?i)^\s*\$(?:(env|global|local|script):)?[A-Za-z_][A-Za-z0-9_]*\s*=\s*([\s\S]*)$");
regex!(POWERSHELL_COLLECTION, r"(?i)^(?:\[ordered\]\s*)?@\s*[{(]");
regex!(DIRECT_COMMAND, r"^(?:@?\(\s*)?([A-Za-z][A-Za-z0-9_.-]*)\b");
regex!(LITERAL_ASSIGNMENT, r"(?s)^([A-Za-z_][A-Za-z0-9_]*)=(.*)$");
regex!(ALIAS_REFERENCE, r"^\$(?:([A-Za-z_][A-Za-z0-9_]*)|\{([A-Za-z_][A-Za-z0-9_]*)(?:\[@\])?\})$");
regex!(BLOCK_STATEMENT, r"(?i)^(?:catch|finally|for|foreach|function|if|param|switch|try|while)\s*[{(]");
regex!(FUNCTION_DEFINITION, r"^[A-Za-z_][A-Za-z0-9_]*\s*\(\s*\)\s*\{");
regex!(HERE_STRING_START, r#"^@["'](?:\r?\n)"#);
regex!(WINDOWS_PATH_COMMAND, r"(?i)^\s*((?:\.{1,2}|%[A-Za-z_][A-Za-z0-9_]*%|\$env:[A-Za-z_][A-Za-z0-9_]*)\\\S+)");
regex!(REDIRECTION, r"^(?:(?:(?:\d+|\*|\{[A-Za-z_][A-Za-z0-9_]*\})?(?:<<<|<<-|<<|<>|>>|>\||<&|>&|<|>))|&>>|&>)(.*)$");
regex!(SCRIPT_OPTIONS, r"^-[adkpqr]+$");
regex!(ARCH_FLAG, r"^-(?:arm64|arm64e|i386|x86_64)$");
regex!(TIMEOUT_DURATION, r"^(?:\d+(?:\.\d*)?|\.\d+)[smhd]?$");
regex!(SHORT_OPTION_CLUSTER, r"^-[A-Za-z].+");
regex!(EXEC_A_ATTACHED, r"^-a.+");
regex!(EXEC_CL, r"^-[cl]+$");
regex!(FUNCTION_TOKEN, r"^[A-Za-z_][A-Za-z0-9_]*\(\)\{$");
regex!(HIGHLIGHT_PROGRAM, r#"^(?:&\s*)?(?:"([^"]*)"|'([^']*)'|(\S+))"#);
regex!(CMDLET, r"^([A-Z][a-z]+(?:[A-Z][a-z]+)?)-[A-Z][A-Za-z]*$");
regex!(SCOPED_VARIABLE, r"(?i)^\$(?:env|global|local|script):");
regex!(VARIABLE_ASSIGNMENT, r"^\$[A-Za-z_]\w*\s+=");
regex!(CALL_START, r"^&\s*\S");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Exec,
    Shell,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrapper {
    Env,
    Sudo,
}

impl Wrapper {
    fn options_with_value(self) -> &'static [&'static str] {
        match self {
            Self::Env => &["-C", "--chdir", "-S", "--split-string", "-u", "--unset"],
            Self::Sudo => &["-C", "--close-from", "-D", "--chdir", "-g", "--group", "-u", "--user"],
        }
    }

    fn flags(self) -> &'static [&'static str] {
        match self {
            Self::Env => &["-0", "--null", "-i", "--ignore-environment", "--debug", "-v"],
            Self::Sudo => &["-A", "--askpass", "-b", "--background", "-E", "-H", "-i", "-n", "-S"],
        }
    }
}

fn last_path_part(value: &str) -> &str {
    value.rsplit(['/', '\\']).next().unwrap_or(value)
}

fn has_path_separator(value: &str) -> bool {
    value.contains(['/', '\\'])
}

fn first_char(value: &str) -> Option<char> {
    value.chars().next()
}

/// The index of the script argument after a POSIX shell's options (`shellCommandArgumentIndex`).
fn shell_command_argument_index(tokens: &[String], start: usize) -> Option<usize> {
    let mut index = start;
    while index < tokens.len() {
        let option = tokens[index].as_str();
        if option == "--" || !option.starts_with('-') {
            return None;
        }
        if SHELL_OPTIONS_WITH_VALUE.contains(&option) {
            index += 2;
            continue;
        }
        let combined_c = option.len() > 1 && option[1..].bytes().all(|b| b.is_ascii_alphabetic()) && option[1..].contains('c');
        if option == "--command" || combined_c {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

fn is_windows_path_prefix(current: &str) -> bool {
    let bytes = current.as_bytes();
    let head_len = if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        2
    } else if current.starts_with("..") {
        2
    } else if current.starts_with('.') {
        1
    } else {
        return false;
    };
    let rest = &current[head_len..];
    rest.is_empty() || (rest.starts_with('\\') && !rest.chars().any(char::is_whitespace))
}

/// Splits a command into words the way a POSIX shell quotes them (`tokenizeShellCommand`).
/// `None` when quoting, escapes or substitutions do not close.
pub fn tokenize_shell_command(command: &str) -> Option<Vec<String>> {
    let input: Vec<char> = command.trim().chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaping = false;
    let mut in_backticks = false;
    let mut substitution_depth = 0usize;
    let mut parameter_depth = 0usize;
    let mut started = false;
    let mut index = 0;
    while index < input.len() {
        let character = input[index];
        let next = input.get(index + 1).copied();
        index += 1;
        if escaping {
            current.push(character);
            escaping = false;
            started = true;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            let windows_path = quote.is_none() && is_windows_path_prefix(&current);
            if (quote == Some('"') || windows_path) && next.is_some_and(|n| !matches!(n, '"' | '\\' | '$' | '`' | '\n')) {
                current.push(character);
                started = true;
                continue;
            }
            escaping = true;
            started = true;
            continue;
        }
        if in_backticks {
            current.push(character);
            if character == '`' {
                in_backticks = false;
            }
            started = true;
            continue;
        }
        if let Some(open) = quote {
            if character == open {
                quote = None;
            } else {
                current.push(character);
            }
            started = true;
            continue;
        }
        match character {
            '`' => {
                current.push(character);
                in_backticks = true;
            }
            '$' if next == Some('{') => {
                current.push_str("${");
                parameter_depth += 1;
                index += 1;
            }
            '{' if parameter_depth > 0 => {
                current.push(character);
                parameter_depth += 1;
            }
            '}' if parameter_depth > 0 => {
                current.push(character);
                parameter_depth -= 1;
            }
            '$' if next == Some('(') => {
                current.push_str("$(");
                substitution_depth += 1;
                index += 1;
            }
            '(' => {
                current.push(character);
                substitution_depth += 1;
            }
            ')' if substitution_depth > 0 => {
                current.push(character);
                substitution_depth -= 1;
            }
            '"' | '\'' => quote = Some(character),
            c if c.is_whitespace() => {
                if substitution_depth > 0 || parameter_depth > 0 {
                    current.push(c);
                } else {
                    if started {
                        tokens.push(std::mem::take(&mut current));
                        started = false;
                    }
                    continue;
                }
            }
            c => current.push(c),
        }
        started = true;
    }
    if quote.is_some() || escaping || in_backticks || substitution_depth > 0 || parameter_depth > 0 {
        return None;
    }
    if started {
        tokens.push(current);
    }
    Some(tokens)
}

/// The first command of a command line and what follows its separator (`splitFirstShellCommand`).
struct ShellCommandSplit {
    first_command: String,
    remaining_command: Option<String>,
    separator: Option<String>,
}

struct Heredoc {
    delimiter: String,
    strip_tabs: bool,
}

fn collect(chars: &[char], start: usize, end: usize) -> String {
    chars[start.min(chars.len())..end.min(chars.len())].iter().collect()
}

fn without_comments(chars: &[char], end: usize, comments: &[(usize, usize)]) -> String {
    let mut result = String::new();
    let mut cursor = 0;
    for &(start, comment_end) in comments {
        if start >= end {
            break;
        }
        result.push_str(&collect(chars, cursor, start));
        cursor = comment_end.min(end);
    }
    result.push_str(&collect(chars, cursor, end));
    result
}

fn read_heredoc_delimiter(chars: &[char], start: usize, strip_tabs: bool) -> Option<(Heredoc, usize)> {
    let mut index = start;
    while matches!(chars.get(index), Some(' ' | '\t')) {
        index += 1;
    }
    let mut delimiter = String::new();
    let mut quote: Option<char> = None;
    let mut escaping = false;
    while index < chars.len() {
        let character = chars[index];
        if escaping {
            delimiter.push(character);
            escaping = false;
        } else if character == '\\' && quote != Some('\'') {
            escaping = true;
        } else if let Some(open) = quote {
            if character == open {
                quote = None;
            } else {
                delimiter.push(character);
            }
        } else if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character.is_whitespace() || ";&|<>()".contains(character) {
            break;
        } else {
            delimiter.push(character);
        }
        index += 1;
    }
    if delimiter.is_empty() || quote.is_some() || escaping {
        return None;
    }
    Some((Heredoc { delimiter, strip_tabs }, index))
}

fn command_after_heredocs(chars: &[char], start: usize, heredocs: &[Heredoc]) -> Option<String> {
    let mut cursor = start;
    for heredoc in heredocs {
        let mut found = false;
        while cursor <= chars.len() {
            let newline = chars[cursor.min(chars.len())..].iter().position(|&c| c == '\n').map(|offset| cursor + offset);
            let line_end = newline.unwrap_or(chars.len());
            let line = collect(chars, cursor, line_end);
            let line = line.strip_suffix('\r').unwrap_or(&line);
            let comparable = if heredoc.strip_tabs { line.trim_start_matches('\t') } else { line };
            cursor = newline.map_or(chars.len(), |n| n + 1);
            if comparable == heredoc.delimiter {
                found = true;
                break;
            }
            if newline.is_none() {
                break;
            }
        }
        if !found {
            return None;
        }
    }
    let rest = collect(chars, cursor, chars.len());
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_owned())
}

fn split_first_shell_command(command: &str) -> ShellCommandSplit {
    let chars: Vec<char> = command.chars().collect();
    let at = |index: usize| chars.get(index).copied();
    let mut quote: Option<char> = None;
    let mut here_string_quote: Option<char> = None;
    let mut escaping = false;
    let mut in_backticks = false;
    let mut in_comment = false;
    let mut substitution_depth = 0usize;
    let mut parameter_depth = 0usize;
    let mut heredocs: Vec<Heredoc> = Vec::new();
    let mut comments: Vec<(usize, usize)> = Vec::new();
    let mut comment_start = 0;
    let mut separator_before_heredocs: Option<(usize, usize)> = None;

    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if let Some(open) = here_string_quote {
            if character == open && at(index + 1) == Some('@') && (index == 0 || at(index - 1) == Some('\n')) {
                here_string_quote = None;
                index += 1;
            }
            index += 1;
            continue;
        }
        if in_comment {
            if character != '\n' {
                index += 1;
                continue;
            }
            in_comment = false;
            comments.push((comment_start, index));
        }
        if escaping {
            escaping = false;
            index += 1;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaping = true;
            index += 1;
            continue;
        }
        if in_backticks {
            if character == '`' {
                in_backticks = false;
            }
            index += 1;
            continue;
        }
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        if character == '@'
            && matches!(at(index + 1), Some('"' | '\''))
            && (at(index + 2) == Some('\n') || (at(index + 2) == Some('\r') && at(index + 3) == Some('\n')))
        {
            here_string_quote = at(index + 1);
            index += 2;
            continue;
        }
        if character == '"' || character == '\'' {
            quote = Some(character);
            index += 1;
            continue;
        }
        if character == '`' {
            in_backticks = true;
            index += 1;
            continue;
        }
        if character == '#' && (index == 0 || at(index - 1).is_some_and(|p| p.is_whitespace() || ";&|(".contains(p))) {
            in_comment = true;
            comment_start = index;
            index += 1;
            continue;
        }
        if character == '$' && at(index + 1) == Some('{') {
            parameter_depth += 1;
            index += 2;
            continue;
        }
        if character == '{' && parameter_depth > 0 {
            parameter_depth += 1;
            index += 1;
            continue;
        }
        if character == '}' && parameter_depth > 0 {
            parameter_depth -= 1;
            index += 1;
            continue;
        }
        if character == '(' {
            substitution_depth += 1;
            index += 1;
            continue;
        }
        if character == ')' && substitution_depth > 0 {
            substitution_depth -= 1;
            index += 1;
            continue;
        }
        if substitution_depth > 0 || parameter_depth > 0 {
            index += 1;
            continue;
        }
        if character == '<' && at(index + 1) == Some('<') && at(index + 2) != Some('<') {
            let strip_tabs = at(index + 2) == Some('-');
            let Some((heredoc, end)) = read_heredoc_delimiter(&chars, index + if strip_tabs { 3 } else { 2 }, strip_tabs) else {
                return ShellCommandSplit { first_command: command.trim().to_owned(), remaining_command: None, separator: None };
            };
            heredocs.push(heredoc);
            index = end;
            continue;
        }
        let double_operator = (character == '&' && at(index + 1) == Some('&'))
            || (character == '|' && matches!(at(index + 1), Some('|' | '&')));
        let redirection_ampersand = character == '&'
            && (matches!(index.checked_sub(1).and_then(at), Some('>' | '<')) || at(index + 1) == Some('>'));
        if (!double_operator && !";&|\n".contains(character)) || redirection_ampersand {
            index += 1;
            continue;
        }
        let length = if double_operator { 2 } else { 1 };
        if character == '\n' && !heredocs.is_empty() {
            let separator = separator_before_heredocs;
            let first_command = without_comments(&chars, separator.map_or(index, |s| s.0), &comments).trim_start().to_owned();
            let before = separator.map(|(start, len)| collect(&chars, start + len, index).trim().to_owned()).unwrap_or_default();
            let following = command_after_heredocs(&chars, index + 1, &heredocs);
            let remaining = [Some(before), following].into_iter().flatten().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n");
            return ShellCommandSplit {
                first_command,
                remaining_command: (!remaining.is_empty()).then_some(remaining),
                separator: Some(separator.map_or_else(|| "\n".to_owned(), |(start, len)| collect(&chars, start, start + len))),
            };
        }
        if !heredocs.is_empty() {
            separator_before_heredocs.get_or_insert((index, length));
            index += length;
            continue;
        }
        let first_command = without_comments(&chars, index, &comments).trim_start().to_owned();
        let mut next = index + length;
        while at(next).is_some_and(char::is_whitespace) {
            next += 1;
        }
        let next_command = collect(&chars, next, chars.len());
        let next_command = next_command.trim();
        return ShellCommandSplit {
            first_command,
            remaining_command: (!next_command.is_empty()).then(|| next_command.to_owned()),
            separator: Some(collect(&chars, index, index + length)),
        };
    }
    if in_comment {
        comments.push((comment_start, chars.len()));
    }
    ShellCommandSplit { first_command: without_comments(&chars, chars.len(), &comments).trim().to_owned(), remaining_command: None, separator: None }
}

fn without_leading_comments(command: &str) -> Option<&str> {
    let mut remaining = command.trim_start();
    while remaining.starts_with('#') {
        let newline = remaining.find('\n')?;
        remaining = remaining[newline + 1..].trim_start();
    }
    (!remaining.is_empty()).then_some(remaining)
}

fn without_line_continuations(command: &str) -> String {
    let chars: Vec<char> = command.chars().collect();
    let mut normalized = String::with_capacity(command.len());
    let mut quote: Option<char> = None;
    let mut escaping = false;
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if escaping {
            normalized.push(character);
            escaping = false;
            index += 1;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            if chars.get(index + 1) == Some(&'\n') {
                index += 2;
                continue;
            }
            if chars.get(index + 1) == Some(&'\r') && chars.get(index + 2) == Some(&'\n') {
                index += 3;
                continue;
            }
            normalized.push(character);
            escaping = true;
            index += 1;
            continue;
        }
        match quote {
            Some(open) if character == open => quote = None,
            None if character == '"' || character == '\'' => quote = Some(character),
            _ => {}
        }
        normalized.push(character);
        index += 1;
    }
    normalized
}

/// The token index after a redirection at `index`; `tokens.len() + 1` when its target is missing.
fn index_after_redirection(tokens: &[String], index: usize) -> Option<usize> {
    let token = tokens.get(index)?;
    if token.starts_with("<(") || token.starts_with(">(") {
        return None;
    }
    let captures = REDIRECTION.captures(token)?;
    if captures.get(1).is_some_and(|m| !m.as_str().is_empty()) {
        return Some(index + 1);
    }
    Some(if tokens.get(index + 1).is_none() { tokens.len() + 1 } else { index + 2 })
}

fn serialize_tokens(tokens: &[String]) -> String {
    tokens.iter().map(|token| format!("'{}'", token.replace('\'', "'\\''"))).collect::<Vec<_>>().join(" ")
}

fn transparent_wrapper_index(wrapper: &str, tokens: &[String], index: usize) -> Option<usize> {
    let token = |offset: usize| tokens.get(index + offset).map(String::as_str);
    match wrapper {
        "bundle" => (token(1) == Some("exec") && token(2).is_some()).then_some(index + 2),
        "nohup" => {
            let target_index = if token(1) == Some("--") { index + 2 } else { index + 1 };
            tokens.get(target_index).filter(|t| !t.starts_with('-')).map(|_| target_index)
        }
        "script" => (SCRIPT_OPTIONS.is_match(token(1).unwrap_or("")) && token(3).is_some()).then_some(index + 3),
        "arch" => {
            if ARCH_FLAG.is_match(token(1).unwrap_or("")) {
                token(2).is_some().then_some(index + 2)
            } else {
                (token(1) == Some("-arch") && token(3).is_some()).then_some(index + 3)
            }
        }
        "timeout" | "gtimeout" => (TIMEOUT_DURATION.is_match(token(1).unwrap_or("")) && token(2).is_some()).then_some(index + 2),
        _ => None,
    }
}

fn static_program_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // A URL scheme (`https:`) is not a program; a drive letter followed by a slash is a path.
    if let Some(scheme) = URL_SCHEME.find(trimmed)
        && !matches!(trimmed[scheme.end()..].chars().next(), Some('/' | '\\'))
    {
        return None;
    }
    let program = last_path_part(trimmed);
    if program.is_empty()
        || (program.chars().any(char::is_whitespace) && !has_path_separator(trimmed))
        || first_char(program).is_some_and(|c| NON_PROGRAM_PREFIX_CHARACTERS.contains(c))
        || program.chars().last().is_some_and(|c| NON_PROGRAM_SUFFIX_CHARACTERS.contains(c))
    {
        return None;
    }
    Some(program.to_owned())
}

fn leading_powershell_literal(command: &str) -> Option<String> {
    let input = command.trim_start();
    let chars: Vec<char> = input.chars().collect();
    let Some(&quote) = chars.first().filter(|c| matches!(c, '"' | '\'')) else {
        return static_program_name(LEADING_WORD.find(input).map_or("", |m| m.as_str()));
    };
    let mut value = String::new();
    let mut index = 1;
    while index < chars.len() {
        let character = chars[index];
        if character == '`' && index + 1 < chars.len() {
            value.push(chars[index + 1]);
            index += 2;
            continue;
        }
        if character == quote {
            if quote == '\'' && chars.get(index + 1) == Some(&'\'') {
                value.push('\'');
                index += 2;
                continue;
            }
            return static_program_name(&value);
        }
        value.push(character);
        index += 1;
    }
    None
}

/// `Some(program)` for a PowerShell `& 'program' …` call, `None` when it is not one.
fn call_operator_program(command: &str) -> Option<Option<String>> {
    CALL_OPERATOR.captures(command).map(|captures| leading_powershell_literal(&captures[1]))
}

fn powershell_assignment_program(command: &str, depth: usize, remaining: Option<&str>, segments: usize) -> Option<Option<String>> {
    let captures = POWERSHELL_ASSIGNMENT.captures(command)?;
    let next = || remaining.and_then(|rest| program_internal(rest, depth, Context::Shell, segments - 1));
    if captures.get(1).is_some_and(|scope| scope.as_str().eq_ignore_ascii_case("env")) {
        return Some(next());
    }
    let value = captures[2].trim();
    // The POSIX-oriented splitter does not balance PowerShell arrays or hashtables.
    if POWERSHELL_COLLECTION.is_match(value) {
        return Some(None);
    }
    if let Some(called) = call_operator_program(value) {
        return Some(called);
    }
    if let Some(direct) = DIRECT_COMMAND.captures(value).map(|c| c[1].to_owned())
        && !NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&direct.to_lowercase().as_str())
    {
        return Some(program_internal(value, depth + 1, Context::Shell, segments).or(Some(direct)));
    }
    Some(next())
}

fn windows_shell_payload_program(
    shell: &str,
    tokens: &[String],
    start: usize,
    depth: usize,
    split: &ShellCommandSplit,
    segments: usize,
) -> Option<Option<String>> {
    let parse = |payload: Option<&String>| -> Option<String> {
        let payload = payload?;
        let command = match (&split.remaining_command, &split.separator) {
            (Some(rest), Some(separator)) => format!("{payload} {separator} {rest}"),
            _ => payload.clone(),
        };
        program_internal(&command, depth + 1, Context::Shell, segments)
    };
    if shell == "cmd" || shell == "cmd.exe" {
        let index = (start..tokens.len()).find(|&i| matches!(tokens[i].to_lowercase().as_str(), "/c" | "/k"))?;
        return Some(parse(tokens.get(index + 1)));
    }
    let mut index = start;
    while index < tokens.len() {
        let option = tokens[index].to_lowercase();
        match option.as_str() {
            "-command" | "-c" => return Some(parse(tokens.get(index + 1))),
            "-file" | "-f" => return Some(static_program_name(tokens.get(index + 1).map_or("", String::as_str))),
            "-encodedcommand" | "-enc" | "-e" => return Some(None),
            o if POWERSHELL_OPTIONS_WITH_VALUE.contains(&o) => index += 1,
            o if POWERSHELL_FLAGS.contains(&o) => {}
            o if !o.starts_with('-') => return Some(static_program_name(&tokens[index])),
            _ => {}
        }
        index += 1;
    }
    None
}

fn start_process_program(tokens: &[String], start: usize) -> Option<String> {
    let mut index = start;
    while index < tokens.len() {
        let token = &tokens[index];
        let option = token.to_lowercase();
        if option == "-filepath" {
            return static_program_name(tokens.get(index + 1).map_or("", String::as_str));
        }
        if START_PROCESS_FLAGS.contains(&option.as_str()) {
            index += 1;
            continue;
        }
        if START_PROCESS_OPTIONS_WITH_VALUE.contains(&option.as_str()) {
            tokens.get(index + 1)?;
            index += 2;
            continue;
        }
        if token.starts_with('-') {
            return None;
        }
        return static_program_name(token);
    }
    None
}

fn literal_assignment_program(token: &str) -> Option<(String, Option<String>)> {
    let captures = LITERAL_ASSIGNMENT.captures(token)?;
    let name = captures[1].to_owned();
    let mut value = captures[2].trim().to_owned();
    if value.is_empty() || value.contains(['$', '`']) {
        return Some((name, None));
    }
    if value.starts_with('(') && value.ends_with(')') && value.len() >= 2 {
        value = tokenize_shell_command(&value[1..value.len() - 1]).and_then(|t| t.into_iter().next()).unwrap_or_default();
    } else if value.chars().any(char::is_whitespace) && !has_path_separator(&value) {
        return Some((name, None));
    }
    let program = static_program_name(&value);
    Some((name, program))
}

fn literal_alias_program(command: &str) -> Option<String> {
    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut remaining = Some(command.to_owned());
    let mut control_depth = 0usize;
    let mut segment = 0;
    while let Some(current) = remaining.take().filter(|_| segment < MAX_COMMAND_SEGMENTS) {
        segment += 1;
        let without = without_leading_comments(&current)?;
        let split = split_first_shell_command(without);
        let tokens = tokenize_shell_command(&without_line_continuations(&split.first_command))?;
        let mut command_index = usize::from(matches!(tokens.first().map(String::as_str), Some("do" | "then")));
        while command_index < tokens.len() {
            if let Some(after) = index_after_redirection(&tokens, command_index)
                && after <= tokens.len()
            {
                command_index = after;
                continue;
            }
            if ASSIGNMENT_PREFIX.is_match(&tokens[command_index]) {
                command_index += 1;
                continue;
            }
            break;
        }
        if let Some(captures) = ALIAS_REFERENCE.captures(tokens.get(command_index).map_or("", String::as_str)) {
            let name = captures.get(1).or_else(|| captures.get(2)).map_or("", |m| m.as_str());
            if let Some(program) = aliases.get(name) {
                return Some(program.clone());
            }
        }
        let leading = tokens.first().map(String::as_str);
        if matches!(leading, Some("fi" | "done" | "esac")) {
            control_depth = control_depth.saturating_sub(1);
        }
        if matches!(leading, Some("if" | "for" | "while" | "until" | "select" | "case")) {
            control_depth += 1;
        }
        if control_depth == 0 && leading == Some("unset") {
            for name in &tokens[1..] {
                aliases.remove(name);
            }
        }
        let start = usize::from(leading == Some("export"));
        let assignments: Option<Vec<_>> = tokens[start.min(tokens.len())..].iter().map(|t| literal_assignment_program(t)).collect();
        if control_depth == 0
            && let Some(assignments) = assignments.filter(|a| !a.is_empty())
        {
            for (name, program) in assignments {
                match program {
                    Some(program) => aliases.insert(name, program),
                    None => aliases.remove(&name),
                };
            }
        }
        remaining = split.remaining_command;
    }
    None
}

fn wrapped_shell_command_program(
    wrapper: &str,
    tokens: &[String],
    start: usize,
    depth: usize,
    remaining: Option<&str>,
    segments: usize,
) -> Option<String> {
    let mut index = start;
    match wrapper {
        "command" => {
            while index < tokens.len() {
                let option = tokens[index].as_str();
                if option == "--" {
                    index += 1;
                    break;
                }
                if !option.starts_with('-') || option == "-" {
                    break;
                }
                if option != "-p" {
                    return None;
                }
                index += 1;
            }
        }
        "builtin" => {
            if tokens.get(index).map(String::as_str) == Some("--") {
                index += 1;
            } else if tokens.get(index).is_some_and(|t| t.starts_with('-')) {
                return None;
            }
        }
        "exec" => {
            while index < tokens.len() {
                let option = tokens[index].as_str();
                if option == "--" {
                    index += 1;
                    break;
                }
                if option == "-a" {
                    tokens.get(index + 1)?;
                    index += 2;
                    continue;
                }
                if EXEC_A_ATTACHED.is_match(option) || EXEC_CL.is_match(option) {
                    index += 1;
                    continue;
                }
                if option.starts_with('-') && option != "-" {
                    return None;
                }
                break;
            }
        }
        _ => {}
    }
    let wrapped = &tokens[index.min(tokens.len())..];
    if wrapped.is_empty() {
        return None;
    }
    let mut target_index = 0;
    while target_index < wrapped.len() {
        match index_after_redirection(wrapped, target_index) {
            Some(after) if after <= wrapped.len() => target_index = after,
            _ => break,
        }
    }
    let target = wrapped.get(target_index).map(String::as_str);
    if target.is_some_and(|t| ASSIGNMENT_PREFIX.is_match(t)) {
        return None;
    }
    let context = if wrapper == "exec" { Context::Exec } else { Context::Shell };
    if let Some(program) = program_internal(&serialize_tokens(wrapped), depth + 1, context, segments) {
        return Some(program);
    }
    match (target, remaining) {
        (Some(target), Some(rest))
            if wrapper != "exec"
                && target == target.to_lowercase()
                && NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&target)
                && !TERMINAL_SHELL_PROGRAMS.contains(&target) =>
        {
            program_internal(rest, depth, Context::Shell, segments - 1)
        }
        _ => None,
    }
}

fn parse_program(command: &str, depth: usize, context: Context, segments: usize) -> Option<String> {
    if depth >= 8 || segments == 0 {
        return None;
    }
    let command = without_leading_comments(command)?;
    if BLOCK_STATEMENT.is_match(command) || FUNCTION_DEFINITION.is_match(command) {
        return None;
    }
    // `&&` and `||` inside `[[ … ]]` are not separators; stay conservative.
    if command.starts_with("[[") {
        return None;
    }
    let split = split_first_shell_command(command);
    let remaining = split.remaining_command.as_deref();
    let next = |context: Context| remaining.and_then(|rest| program_internal(rest, depth, context, segments - 1));
    if HERE_STRING_START.is_match(split.first_command.trim_start()) {
        return next(Context::Shell);
    }
    if let Some(program) = powershell_assignment_program(&split.first_command, depth, remaining, segments) {
        return program;
    }
    if let Some(path) = WINDOWS_PATH_COMMAND.captures(&split.first_command) {
        return static_program_name(&path[1]);
    }
    let tokens = tokenize_shell_command(&without_line_continuations(&split.first_command))?;
    let first_character = first_char(split.first_command.trim_start());
    if tokens.len() == 1
        && matches!(first_character, Some('"' | '\''))
        && tokens[0].contains(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '=')
        && !has_path_separator(&tokens[0])
    {
        return next(context);
    }
    let mut index = 0;
    let mut wrapper: Option<Wrapper> = None;
    let mut context = context;
    let mut saw_assignment = false;
    let mut saw_redirection = false;

    while index < tokens.len() {
        let token = tokens[index].as_str();
        if token.is_empty() {
            return None;
        }
        if let Some(after) = index_after_redirection(&tokens, index) {
            if after > tokens.len() {
                return None;
            }
            saw_redirection = true;
            index = after;
            continue;
        }
        if ASSIGNMENT_PREFIX.is_match(token) {
            saw_assignment = true;
            index += 1;
            continue;
        }
        if context == Context::Shell && token == ":" {
            return next(context);
        }
        if first_char(token).is_some_and(|c| NON_PROGRAM_PREFIX_CHARACTERS.contains(c)) && !(token.starts_with('$') && token.contains('/')) {
            return if context == Context::Shell && token.starts_with('[') { next(context) } else { None };
        }
        let program = last_path_part(token);
        let unqualified = token == program;
        if program == "env" || program == "sudo" {
            wrapper = Some(if program == "env" { Wrapper::Env } else { Wrapper::Sudo });
            context = Context::Exec;
            index += 1;
            continue;
        }
        if let Some(active) = wrapper {
            if token == "--" {
                wrapper = None;
                index += 1;
                continue;
            }
            if token.starts_with('-') {
                if active == Wrapper::Env && (token == "-S" || token == "--split-string") {
                    return tokens.get(index + 1).and_then(|split| program_internal(split, depth + 1, context, segments));
                }
                if active == Wrapper::Env
                    && let Some(split) = token.strip_prefix("--split-string=")
                {
                    return program_internal(split, depth + 1, context, segments);
                }
                if active.options_with_value().contains(&token) {
                    tokens.get(index + 1)?;
                    index += 2;
                    continue;
                }
                if active.flags().contains(&token) {
                    index += 1;
                    continue;
                }
                if token.starts_with("--")
                    && let Some(equals) = token.find('=').filter(|&e| e > 2)
                {
                    if !active.options_with_value().contains(&&token[..equals]) {
                        return None;
                    }
                    index += 1;
                    continue;
                }
                if SHORT_OPTION_CLUSTER.is_match(token) && !token.starts_with("--") {
                    let cluster: Vec<char> = token[1..].chars().collect();
                    let mut consumes_next = false;
                    for (position, option) in cluster.iter().enumerate() {
                        let short = format!("-{option}");
                        if active.options_with_value().contains(&short.as_str()) {
                            consumes_next = position == cluster.len() - 1;
                            break;
                        }
                        if !active.flags().contains(&short.as_str()) {
                            return None;
                        }
                    }
                    if consumes_next && tokens.get(index + 1).is_none() {
                        return None;
                    }
                    index += if consumes_next { 2 } else { 1 };
                    continue;
                }
                return None;
            }
        }
        let shell_name = program.to_lowercase();
        let shell_name = shell_name.strip_suffix(".exe").unwrap_or(&shell_name);
        if SHELL_PROGRAMS.contains(&shell_name)
            && let Some(script_index) = shell_command_argument_index(&tokens, index + 1)
        {
            return tokens.get(script_index).and_then(|script| program_internal(script, depth + 1, Context::Shell, segments));
        }
        let lower = program.to_lowercase();
        if WINDOWS_SHELL_PROGRAMS.contains(&lower.as_str())
            && let Some(payload) = windows_shell_payload_program(&lower, &tokens, index + 1, depth, &split, segments)
        {
            return payload;
        }
        if lower == "start-process"
            && let Some(started) = start_process_program(&tokens, index + 1)
        {
            return Some(started);
        }
        if context == Context::Shell && unqualified && SHELL_PRECOMMAND_MODIFIERS.contains(&program) {
            index += 1;
            if program == "time" && tokens.get(index).map(String::as_str) == Some("-p") {
                index += 1;
            }
            if tokens.get(index).is_some_and(|t| t.starts_with('-')) {
                return None;
            }
            continue;
        }
        if unqualified
            && let Some(target) = transparent_wrapper_index(program, &tokens, index)
            && let Some(wrapped) = program_internal(&serialize_tokens(&tokens[target..]), depth + 1, Context::Exec, segments)
        {
            return Some(wrapped);
        }
        if unqualified && SHELL_COMMAND_WRAPPERS.contains(&program) {
            return wrapped_shell_command_program(program, &tokens, index + 1, depth, remaining, segments);
        }
        if (context == Context::Shell || (wrapper == Some(Wrapper::Sudo) && SKIPPABLE_SUDO_PROBES.contains(&program)))
            && unqualified
            && NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&program)
            && (!TERMINAL_SHELL_PROGRAMS.contains(&program) || (program == "false" && split.separator.as_deref() != Some("&&")))
            && remaining.is_some()
        {
            return next(Context::Shell);
        }
        if context == Context::Shell && unqualified && POWERSHELL_SETUP_PROGRAMS.contains(&lower.as_str()) && remaining.is_some() {
            return next(Context::Shell);
        }
        if program.is_empty()
            || (unqualified && NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&program))
            || first_char(program).is_some_and(|c| NON_PROGRAM_PREFIX_CHARACTERS.contains(c))
            || program.chars().last().is_some_and(|c| NON_PROGRAM_SUFFIX_CHARACTERS.contains(c))
            || program.ends_with("()")
            || FUNCTION_TOKEN.is_match(program)
        {
            return None;
        }
        return Some(program.to_owned());
    }
    if (saw_assignment || saw_redirection) && wrapper.is_none() {
        return next(context);
    }
    None
}

fn program_internal(command: &str, depth: usize, context: Context, segments: usize) -> Option<String> {
    if segments == 0 {
        return None;
    }
    if let Some(called) = call_operator_program(command) {
        return called;
    }
    parse_program(command, depth, context, segments).or_else(|| if context == Context::Shell { literal_alias_program(command) } else { None })
}

/// The program a shell command runs, for "Ran pnpm" labels: `bash -lc 'pnpm test'` → `pnpm`
/// (`commandProgramName`). `None` when no program can be named with confidence.
pub fn command_program_name(command: &str) -> Option<String> {
    program_internal(command, 0, Context::Shell, MAX_COMMAND_SEGMENTS)
}

fn windows_shell_script_index(shell: &str, tokens: &[String]) -> Option<usize> {
    let mut index = 1;
    while index < tokens.len() {
        let option = tokens[index].to_lowercase();
        if shell == "cmd" {
            if option == "/c" {
                return Some(index + 1);
            }
            if !option.starts_with('/') {
                return None;
            }
            index += 1;
            continue;
        }
        if option == "-command" || option == "-c" {
            return Some(index + 1);
        }
        if POWERSHELL_OPTIONS_WITH_VALUE.contains(&option.as_str()) {
            index += 2;
            continue;
        }
        if !POWERSHELL_FLAGS.contains(&option.as_str()) {
            return None;
        }
        index += 1;
    }
    None
}

/// The script of a lone shell wrapper, or the trimmed command (`withoutShellWrapper`). A
/// wrapper whose quoting does not balance is kept: its script's end cannot be known.
pub fn without_shell_wrapper(command: &str) -> String {
    let trimmed = command.trim();
    if split_first_shell_command(trimmed).remaining_command.is_some() {
        return trimmed.to_owned();
    }
    let Some(tokens) = tokenize_shell_command(trimmed) else { return trimmed.to_owned() };
    let Some(executable) = tokens.first().map(|first| last_path_part(first)) else { return trimmed.to_owned() };
    let program = if executable.to_lowercase().ends_with(".exe") { &executable[..executable.len() - 4] } else { executable };
    if executable.is_empty() || program.is_empty() {
        return trimmed.to_owned();
    }
    let script_index = if SHELL_PROGRAMS.contains(&program) {
        shell_command_argument_index(&tokens, 1)
    } else if WINDOWS_SHELL_PROGRAMS.contains(&executable.to_lowercase().as_str()) {
        windows_shell_script_index(&program.to_lowercase(), &tokens)
    } else {
        None
    };
    // Positional arguments can affect the script; keep those invocations intact.
    match script_index {
        Some(index) if index == tokens.len() - 1 => {
            let script = tokens[index].trim();
            if script.is_empty() { trimmed.to_owned() } else { script.to_owned() }
        }
        _ => trimmed.to_owned(),
    }
}

/// Control characters as Unicode control pictures (ESC becomes ␛); tab, newline and carriage
/// return stay (`withVisibleControlCharacters`).
pub fn with_visible_control_characters(text: &str) -> std::borrow::Cow<'_, str> {
    let visible = |c: char| matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}');
    if !text.chars().any(visible) {
        return text.into();
    }
    text.chars()
        .map(|c| match c {
            '\u{7f}' => '␡',
            c if visible(c) => char::from_u32(0x2400 + c as u32).unwrap_or(c),
            c => c,
        })
        .collect::<String>()
        .into()
}

/// The command a row shows (`commandDisplayText`).
pub fn command_display_text(command: &str) -> String {
    with_visible_control_characters(&without_shell_wrapper(command)).into_owned()
}

/// The grammar a command's text highlights with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandLanguage {
    PowerShell,
    ShellScript,
}

fn is_powershell_cmdlet(word: &str) -> bool {
    CMDLET.captures(word).is_some_and(|c| POWERSHELL_VERBS.split(' ').any(|verb| verb == &c[1]))
}

fn is_powershell_script(command: &str) -> bool {
    let trimmed = command.trim();
    if CALL_START.is_match(trimmed) {
        return true;
    }
    let mut rest = Some(trimmed.to_owned());
    for _ in 0..MAX_COMMAND_SEGMENTS {
        let Some(current) = rest.take().filter(|r| !r.is_empty()) else { break };
        let split = split_first_shell_command(&current);
        let statement = split.first_command.trim();
        if SCOPED_VARIABLE.is_match(statement)
            || VARIABLE_ASSIGNMENT.is_match(statement)
            || is_powershell_cmdlet(LEADING_WORD.find(statement).map_or("", |m| m.as_str()))
        {
            return true;
        }
        rest = split.remaining_command;
    }
    false
}

/// Whether a command highlights as PowerShell or shell script (`commandHighlightLanguage`).
pub fn command_highlight_language(command: &str) -> CommandLanguage {
    let name = HIGHLIGHT_PROGRAM
        .captures(command.trim())
        .and_then(|c| c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3)).map(|m| last_path_part(m.as_str()).to_lowercase()));
    if let Some(name) = name
        && matches!(name.strip_suffix(".exe").unwrap_or(&name), "powershell" | "pwsh")
    {
        return CommandLanguage::PowerShell;
    }
    if is_powershell_script(command) { CommandLanguage::PowerShell } else { CommandLanguage::ShellScript }
}
