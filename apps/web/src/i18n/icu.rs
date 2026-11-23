//! ICU MessageFormat parser matching `@generaltranslation/icu`, the parser behind
//! `gt-react`'s `gtFallback`. Pure `std`: `t3code-i18n-macros` includes this file to
//! check `t!` sources at compile time, and the runtime formats parsed messages.
//!
//! Supported: literals with ICU apostrophe quoting, `{name}`, `#` inside plurals,
//! `{n, plural, …}` (with `offset:` and `=N` selectors) and `{v, select, …}`. The
//! catalogs use nothing else; `number`, `date`, `time`, `selectordinal` and tags
//! are parse errors, which callers treat the way gt does: fall back to the source.

use std::borrow::Cow;

/// One parsed piece of a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Node<'a> {
    Text(Cow<'a, str>),
    /// `{name}`, printed as the argument's plain string form.
    Arg(&'a str),
    /// `#` inside a plural branch: the plural value, number-formatted for the locale.
    Pound,
    Select {
        arg: &'a str,
        options: Vec<(&'a str, Vec<Node<'a>>)>,
    },
    Plural {
        arg: &'a str,
        offset: f64,
        /// Selectors are a plural category (`one`) or an exact match (`=1`).
        options: Vec<(&'a str, Vec<Node<'a>>)>,
    },
}

/// Why a message did not parse. The position is a byte offset into the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub code: &'static str,
    pub offset: usize,
}

/// Parses a whole ICU message.
pub fn parse(message: &str) -> Result<Vec<Node<'_>>, ParseError> {
    let mut parser = Parser { src: message, pos: 0 };
    let nodes = parser.message(false, false)?;
    Ok(nodes)
}

/// Every argument name a message reads, in first-use order, with whether it is
/// used as a plural count. `t!` uses this to check its named values.
pub fn argument_names<'a>(nodes: &[Node<'a>], out: &mut Vec<(&'a str, bool)>) {
    let push = |name: &'a str, plural: bool, out: &mut Vec<(&'a str, bool)>| {
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 |= plural,
            None => out.push((name, plural)),
        }
    };
    for node in nodes {
        match node {
            Node::Text(_) | Node::Pound => {}
            Node::Arg(name) => push(name, false, out),
            Node::Select { arg, options } => {
                push(arg, false, out);
                for (_, branch) in options {
                    argument_names(branch, out);
                }
            }
            Node::Plural { arg, options, .. } => {
                push(arg, true, out);
                for (_, branch) in options {
                    argument_names(branch, out);
                }
            }
        }
    }
}

/// True when the message is plain text that formats to itself, so a lookup can
/// return it without parsing. Conservative: any brace or apostrophe says no.
pub fn is_verbatim(message: &str) -> bool {
    !message.bytes().any(|b| matches!(b, b'{' | b'}' | b'\'' | b'<'))
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn current(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek(&self) -> Option<char> {
        let mut chars = self.src[self.pos..].chars();
        chars.next();
        chars.next()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn fail<T>(&self, code: &'static str) -> Result<T, ParseError> {
        Err(ParseError {
            code,
            offset: self.pos,
        })
    }

    fn consume(&mut self, token: &str) -> bool {
        if self.src[self.pos..].starts_with(token) {
            self.pos += token.len();
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) {
        while let Some(c) = self.current() {
            if !is_grammar_white_space(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }

    fn read_identifier(&mut self) -> &'a str {
        let start = self.pos;
        while let Some(c) = self.current() {
            if is_identifier_boundary(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
        &self.src[start..self.pos]
    }

    fn message(&mut self, nested: bool, in_plural: bool) -> Result<Vec<Node<'a>>, ParseError> {
        let mut nodes = Vec::new();
        while let Some(c) = self.current() {
            match c {
                '{' => nodes.push(self.argument()?),
                '}' if nested => break,
                '#' if in_plural => {
                    self.pos += 1;
                    nodes.push(Node::Pound);
                }
                '<' if self.peek().is_some_and(|n| n == '/' || n.is_ascii_alphabetic()) => {
                    return self.fail("UNSUPPORTED_TAG");
                }
                _ => nodes.push(self.literal(nested, in_plural)),
            }
        }
        Ok(nodes)
    }

    fn literal(&mut self, nested: bool, in_plural: bool) -> Node<'a> {
        let src = self.src;
        let start = self.pos;
        // Borrow the source until a quote forces an owned copy.
        let mut owned: Option<String> = None;
        while let Some(c) = self.current() {
            if c == '\'' {
                let before = self.pos;
                if let Some(quoted) = self.quote(in_plural) {
                    owned
                        .get_or_insert_with(|| src[start..before].to_owned())
                        .push_str(&quoted);
                    continue;
                }
            }
            let stop = c == '{'
                || (c == '}' && nested)
                || (c == '#' && in_plural)
                || (c == '<' && self.peek().is_some_and(|n| n == '/' || n.is_ascii_alphabetic()));
            if stop {
                break;
            }
            if let Some(text) = owned.as_mut() {
                text.push(c);
            }
            self.pos += c.len_utf8();
        }
        match owned {
            Some(text) => Node::Text(Cow::Owned(text)),
            None => Node::Text(Cow::Borrowed(&src[start..self.pos])),
        }
    }

    /// Handles an apostrophe at the cursor. Returns the text it stands for, or
    /// `None` when it is a plain apostrophe to copy as-is.
    fn quote(&mut self, in_plural: bool) -> Option<String> {
        let next = self.peek();
        if next == Some('\'') {
            self.pos += 2;
            return Some("'".to_owned());
        }
        let starts_quote = matches!(next, Some('{' | '}' | '<' | '>')) || (in_plural && next == Some('#'));
        if !starts_quote {
            return None;
        }
        self.pos += 1;
        let mut value = String::new();
        while let Some(c) = self.current() {
            if c == '\'' {
                if self.peek() == Some('\'') {
                    value.push('\'');
                    self.pos += 2;
                    continue;
                }
                self.pos += 1;
                break;
            }
            value.push(c);
            self.pos += c.len_utf8();
        }
        Some(value)
    }

    fn argument(&mut self) -> Result<Node<'a>, ParseError> {
        self.pos += 1;
        self.skip_space();
        if self.at_end() {
            return self.fail("EXPECT_ARGUMENT_CLOSING_BRACE");
        }
        if self.current() == Some('}') {
            return self.fail("EMPTY_ARGUMENT");
        }
        let name = self.read_identifier();
        if name.is_empty() {
            return self.fail("MALFORMED_ARGUMENT");
        }
        self.skip_space();
        if self.consume("}") {
            return Ok(Node::Arg(name));
        }
        if !self.consume(",") {
            return self.fail("MALFORMED_ARGUMENT");
        }
        self.skip_space();
        let kind = self.read_identifier();
        match kind {
            "plural" | "select" => self.choice(name, kind == "select"),
            "" => self.fail("EXPECT_ARGUMENT_TYPE"),
            _ => self.fail("UNSUPPORTED_ARGUMENT_TYPE"),
        }
    }

    fn choice(&mut self, name: &'a str, select: bool) -> Result<Node<'a>, ParseError> {
        self.skip_space();
        if !self.consume(",") {
            return self.fail("EXPECT_SELECT_ARGUMENT_OPTIONS");
        }
        self.skip_space();
        let mut offset = 0.0;
        let mut selector = self.read_identifier();
        if !select && selector == "offset" {
            if !self.consume(":") {
                return self.fail("EXPECT_PLURAL_ARGUMENT_OFFSET_VALUE");
            }
            self.skip_space();
            offset = self.integer_token()?.parse::<f64>().unwrap_or(0.0);
            self.skip_space();
            selector = self.read_identifier();
        }
        let mut options: Vec<(&'a str, Vec<Node<'a>>)> = Vec::new();
        loop {
            if selector.is_empty() && !select && self.current() == Some('=') {
                let start = self.pos;
                self.pos += 1;
                self.integer_token()?;
                selector = &self.src[start..self.pos];
            }
            if selector.is_empty() {
                break;
            }
            if options.iter().any(|(s, _)| *s == selector) {
                return self.fail("DUPLICATE_SELECTOR");
            }
            self.skip_space();
            if !self.consume("{") {
                return self.fail("EXPECT_SELECTOR_FRAGMENT");
            }
            let value = self.message(true, !select)?;
            if !self.consume("}") {
                return self.fail("EXPECT_ARGUMENT_CLOSING_BRACE");
            }
            options.push((selector, value));
            self.skip_space();
            selector = self.read_identifier();
        }
        if options.is_empty() {
            return self.fail("EXPECT_SELECTOR");
        }
        if !options.iter().any(|(s, _)| *s == "other") {
            return self.fail("MISSING_OTHER_CLAUSE");
        }
        if !self.consume("}") {
            return self.fail("EXPECT_ARGUMENT_CLOSING_BRACE");
        }
        Ok(if select {
            Node::Select { arg: name, options }
        } else {
            Node::Plural {
                arg: name,
                offset,
                options,
            }
        })
    }

    fn integer_token(&mut self) -> Result<&'a str, ParseError> {
        let start = self.pos;
        if matches!(self.current(), Some('+' | '-')) {
            self.pos += 1;
        }
        let digits = self.pos;
        while self.current().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if digits == self.pos {
            return self.fail("EXPECT_INTEGER");
        }
        Ok(&self.src[start..self.pos])
    }
}

/// `GRAMMAR_WHITE_SPACE` in `@generaltranslation/icu`.
fn is_grammar_white_space(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ' | '\u{85}' | '\u{200E}' | '\u{200F}' | '\u{2028}' | '\u{2029}')
}

/// `IDENTIFIER_BOUNDARY` in `@generaltranslation/icu`: an identifier ends at any of these.
fn is_identifier_boundary(c: char) -> bool {
    matches!(c,
        '\t'..='\r' | ' ' | '\u{85}' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}'
        | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
        | '\u{21}'..='\u{2F}' | '\u{3A}'..='\u{40}' | '\u{5B}'..='\u{5E}' | '\u{60}'
        | '\u{7B}'..='\u{7E}' | '\u{A1}'..='\u{A7}' | '\u{A9}' | '\u{AB}' | '\u{AC}' | '\u{AE}'
        | '\u{B0}' | '\u{B1}' | '\u{B6}' | '\u{BB}' | '\u{BF}' | '\u{D7}' | '\u{F7}'
        | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{203E}' | '\u{2041}'..='\u{2053}'
        | '\u{2055}'..='\u{205E}' | '\u{2190}'..='\u{245F}' | '\u{2500}'..='\u{2775}'
        | '\u{2794}'..='\u{2BFF}' | '\u{2E00}'..='\u{2E7F}' | '\u{3001}'..='\u{3003}'
        | '\u{3008}'..='\u{3020}' | '\u{3030}' | '\u{FD3E}' | '\u{FD3F}' | '\u{FE45}' | '\u{FE46}')
}
