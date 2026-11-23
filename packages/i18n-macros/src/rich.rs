//! The `t_rich!` markup: a compact spelling of a `<T>` block's JSX children.
//!
//! - Text is copied as-is. `{" "}` (any quoted string) is a separate text child,
//!   the way JSX splits `Copy{" "}<Branch>` into `["Copy", " ", …]`.
//! - `{name}` is a `<Var>`; `{name:rt}` a relative-time variable.
//! - `<tag>…</tag>` and `<tag/>` are elements. The tag name reaches the view.
//! - `<Branch:arg mr="MR" cr="change request">PR</Branch>` picks a branch by the
//!   string value of `arg`, falling back to its children.

use crate::key::json_string;

/// One JSX child of a `<T>` block.
pub enum Child {
    Text(String),
    Var {
        name: String,
        kind: &'static str,
        index: u32,
    },
    Element {
        tag: String,
        index: u32,
        children: Vec<Child>,
        branch: Option<Branch>,
    },
}

pub struct Branch {
    pub arg: String,
    pub options: Vec<(String, String)>,
}

/// Parses `t_rich!` markup into children, numbering elements and variables
/// depth-first from 1 as gt-react does.
pub fn parse(markup: &str) -> Result<Vec<Child>, String> {
    let mut parser = Parser {
        src: markup,
        pos: 0,
        next_index: 1,
    };
    let children = parser.children(None)?;
    if parser.pos < markup.len() {
        return Err(format!("unexpected `{}`", &markup[parser.pos..]));
    }
    Ok(children)
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    next_index: u32,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn children(&mut self, closing: Option<&str>) -> Result<Vec<Child>, String> {
        let mut children = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, children: &mut Vec<Child>| {
            if !text.is_empty() {
                children.push(Child::Text(std::mem::take(text)));
            }
        };
        while let Some(c) = self.rest().chars().next() {
            if self.rest().starts_with("</") {
                let Some(tag) = closing else {
                    return Err("closing tag with no open tag".into());
                };
                let expected = format!("</{tag}>");
                if !self.rest().starts_with(&expected) {
                    return Err(format!("expected {expected}"));
                }
                self.pos += expected.len();
                flush(&mut text, &mut children);
                return Ok(children);
            }
            match c {
                '<' => {
                    flush(&mut text, &mut children);
                    children.push(self.element()?);
                }
                '{' => {
                    flush(&mut text, &mut children);
                    children.push(self.expression()?);
                }
                _ => {
                    text.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
        if let Some(tag) = closing {
            return Err(format!("missing </{tag}>"));
        }
        flush(&mut text, &mut children);
        Ok(children)
    }

    fn take_while(&mut self, keep: impl Fn(char) -> bool) -> String {
        let len = self.rest().find(|c| !keep(c)).unwrap_or(self.rest().len());
        let out = self.rest()[..len].to_owned();
        self.pos += len;
        out
    }

    fn skip_space(&mut self) {
        self.take_while(char::is_whitespace);
    }

    fn expect(&mut self, token: &str) -> Result<(), String> {
        if self.rest().starts_with(token) {
            self.pos += token.len();
            Ok(())
        } else {
            Err(format!("expected `{token}` at `{}`", self.rest()))
        }
    }

    fn quoted(&mut self) -> Result<String, String> {
        self.expect("\"")?;
        let mut out = String::new();
        let mut chars = self.rest().char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.pos += i + 1;
                    return Ok(out);
                }
                '\\' => match chars.next() {
                    Some((_, e)) => out.push(e),
                    None => break,
                },
                _ => out.push(c),
            }
        }
        Err("unterminated string".into())
    }

    fn expression(&mut self) -> Result<Child, String> {
        self.expect("{")?;
        if self.rest().starts_with('"') {
            let text = self.quoted()?;
            self.expect("}")?;
            return Ok(Child::Text(text));
        }
        let name = self.take_while(|c| c.is_alphanumeric() || c == '_');
        if name.is_empty() {
            return Err("expected a variable name after `{`".into());
        }
        let kind = if self.rest().starts_with(':') {
            self.pos += 1;
            match self.take_while(|c| c.is_ascii_alphabetic()).as_str() {
                "rt" => "rt",
                other => return Err(format!("unknown variable kind `{other}`")),
            }
        } else {
            "v"
        };
        self.expect("}")?;
        let index = self.next_index;
        self.next_index += 1;
        Ok(Child::Var { name, kind, index })
    }

    fn element(&mut self) -> Result<Child, String> {
        self.expect("<")?;
        let tag = self.take_while(|c| c.is_alphanumeric() || c == '_' || c == '.');
        if tag.is_empty() {
            return Err("expected a tag name after `<`".into());
        }
        let index = self.next_index;
        self.next_index += 1;
        let branch = if tag == "Branch" {
            self.expect(":")?;
            let arg = self.take_while(|c| c.is_alphanumeric() || c == '_');
            let mut options = Vec::new();
            loop {
                self.skip_space();
                let key = self.take_while(|c| c.is_alphanumeric() || c == '_' || c == '-');
                if key.is_empty() {
                    break;
                }
                self.expect("=")?;
                options.push((key, self.quoted()?));
            }
            Some(Branch { arg, options })
        } else {
            None
        };
        self.skip_space();
        if self.rest().starts_with("/>") {
            self.pos += 2;
            return Ok(Child::Element {
                tag,
                index,
                children: Vec::new(),
                branch,
            });
        }
        self.expect(">")?;
        let children = self.children(Some(&tag))?;
        Ok(Child::Element {
            tag,
            index,
            children,
            branch,
        })
    }
}

/// The children as gt stores them in `en.json`, which the runtime parses for the
/// English rendering.
pub fn catalog_json(children: &[Child]) -> String {
    match children {
        [one] => child_json(one, false),
        many => list_json(many, false),
    }
}

/// The children after gt's `sanitizeJsxChildren`, with sorted keys: the hash input.
pub fn sanitized_json(children: &[Child]) -> String {
    match children {
        [one] => child_json(one, true),
        many => list_json(many, true),
    }
}

fn list_json(children: &[Child], sanitized: bool) -> String {
    let parts: Vec<String> = children.iter().map(|c| child_json(c, sanitized)).collect();
    format!("[{}]", parts.join(","))
}

fn child_json(child: &Child, sanitized: bool) -> String {
    match child {
        Child::Text(text) => json_string(text),
        Child::Var { kind, index, .. } => {
            let k = json_string(&format!("_gt_value_{index}"));
            if sanitized {
                format!("{{\"k\":{k},\"v\":\"{kind}\"}}")
            } else {
                format!("{{\"i\":{index},\"k\":{k},\"v\":\"{kind}\"}}")
            }
        }
        Child::Element {
            tag,
            index,
            children,
            branch,
        } => {
            let mut fields = Vec::new();
            let branches = branch.as_ref().map(|b| {
                let mut sorted: Vec<&(String, String)> = b.options.iter().collect();
                // stableStringify sorts object keys; the catalog keeps source order.
                if sanitized {
                    sorted.sort_by(|a, b| a.0.cmp(&b.0));
                }
                let options: Vec<String> = sorted
                    .into_iter()
                    .map(|(k, v)| format!("{}:{}", json_string(k), json_string(v)))
                    .collect();
                format!("{{{}}}", options.join(","))
            });
            if !sanitized {
                fields.push(format!("\"t\":{}", json_string(tag)));
                fields.push(format!("\"i\":{index}"));
                if let Some(b) = &branches {
                    fields.push(format!("\"d\":{{\"b\":{b},\"t\":\"b\"}}"));
                }
            } else if let Some(b) = &branches {
                fields.push(format!("\"b\":{b}"));
            }
            if !children.is_empty() {
                let c = match children.as_slice() {
                    [one] => child_json(one, sanitized),
                    many => list_json(many, sanitized),
                };
                fields.push(format!("\"c\":{c}"));
            }
            if sanitized && branches.is_some() {
                fields.push("\"t\":\"b\"".into());
            }
            format!("{{{}}}", fields.join(","))
        }
    }
}

/// Every variable and branch argument, depth-first.
pub fn arguments(children: &[Child], vars: &mut Vec<(String, u32)>, branches: &mut Vec<(String, u32)>) {
    for child in children {
        match child {
            Child::Text(_) => {}
            Child::Var { name, index, .. } => vars.push((name.clone(), *index)),
            Child::Element {
                index,
                children,
                branch,
                ..
            } => {
                if let Some(b) = branch {
                    branches.push((b.arg.clone(), *index));
                }
                arguments(children, vars, branches);
            }
        }
    }
}
