//! Command text as a work row shows it (`commandLabel.ts`): the script inside a lone shell
//! wrapper (`bash -lc '…'`, `pwsh -Command …`, `cmd /c …`), with control characters drawn as
//! Unicode control pictures.
//!
//! Differs from the TypeScript: the wrapper is kept whenever the tokenized command has any
//! token after the script (the TypeScript splits on shell separators first; a separator outside
//! quotes is a token here, so both keep the wrapper). Program-name extraction for labels is
//! `command_program` in the web crate.

const SHELL_PROGRAMS: &[&str] = &["sh", "bash", "zsh", "dash", "ash", "ksh", "fish"];
const WINDOWS_SHELL_PROGRAMS: &[&str] = &["cmd", "cmd.exe", "powershell", "powershell.exe", "pwsh", "pwsh.exe"];
const SHELL_OPTIONS_WITH_VALUE: &[&str] = &["-o", "-O", "--rcfile", "--init-file"];
const POWERSHELL_FLAGS: &[&str] = &["-mta", "-nologo", "-noninteractive", "-noprofile", "-sta"];
const POWERSHELL_OPTIONS_WITH_VALUE: &[&str] =
    &["-configurationname", "-executionpolicy", "-inputformat", "-outputformat", "-version", "-windowstyle", "-workingdirectory"];

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
        let combined_c = option.len() > 1
            && option.as_bytes()[0] == b'-'
            && option[1..].bytes().all(|b| b.is_ascii_alphabetic())
            && option[1..].contains('c');
        if option == "--command" || combined_c {
            return Some(index + 1);
        }
        index += 1;
    }
    None
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

/// The script of a lone shell wrapper, or the command itself (`withoutShellWrapper`).
pub fn without_shell_wrapper(command: &str) -> &str {
    let trimmed = command.trim();
    let Some(tokens) = tokenize_shell_command(trimmed) else { return trimmed };
    let Some(executable) = tokens.first().and_then(|first| first.rsplit(['/', '\\']).next()) else { return trimmed };
    let program = executable.strip_suffix(".exe").or_else(|| executable.strip_suffix(".EXE")).unwrap_or(executable);
    if program.is_empty() {
        return trimmed;
    }
    let script_index = if SHELL_PROGRAMS.contains(&program) {
        shell_command_argument_index(&tokens, 1)
    } else if WINDOWS_SHELL_PROGRAMS.contains(&executable.to_lowercase().as_str()) {
        windows_shell_script_index(&program.to_lowercase(), &tokens)
    } else {
        None
    };
    match script_index {
        Some(index) if index == tokens.len() - 1 => {
            let script = tokens[index].trim();
            if script.is_empty() {
                return trimmed;
            }
            // The script is a substring of the command once quotes are removed; find it so the
            // result borrows. Escaped scripts differ from their source and fall back to the whole.
            trimmed.find(script).map_or(trimmed, |start| &trimmed[start..start + script.len()])
        }
        _ => trimmed,
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
    with_visible_control_characters(without_shell_wrapper(command)).into_owned()
}
