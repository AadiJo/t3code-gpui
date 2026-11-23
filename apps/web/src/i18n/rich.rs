//! `<T>` blocks: gt-react stores their JSX children as arrays of strings, variables
//! (`{"k": "_gt_value_1", "v": "v"}`) and elements (`{"t": "code", "i": 1, "c": …}`).
//! `t_rich!` renders one into styled segments a view can lay out.

use std::sync::{Arc, OnceLock};

use gpui_kit::{HighlightStyle, SharedString, StyledText};
use serde_json::Value;

use super::{
    catalog::CatalogValue,
    messages::{Arg, active, js_number_string},
};

/// One node of a `<T>` children tree.
#[derive(Debug)]
pub enum RichNode {
    Text(SharedString),
    /// A variable, by its gt key (`_gt_value_1`).
    Var(SharedString),
    Element {
        /// gt's element index; translations refer to source elements by it.
        index: u32,
        tag: SharedString,
        children: Box<[RichNode]>,
        /// `<Branch>` options; empty for every other element.
        branches: Box<[(SharedString, Box<[RichNode]>)]>,
    },
}

impl RichNode {
    /// Converts children as stored in a catalog. Unknown shapes are skipped.
    pub fn from_json_children(children: &[Value]) -> Vec<RichNode> {
        children.iter().filter_map(Self::from_json).collect()
    }

    fn from_json(value: &Value) -> Option<RichNode> {
        match value {
            Value::String(text) => Some(RichNode::Text(text.as_str().into())),
            Value::Object(object) => {
                if let Some(Value::String(k)) = object.get("k") {
                    return Some(RichNode::Var(k.as_str().into()));
                }
                let children = match object.get("c") {
                    Some(Value::Array(list)) => Self::from_json_children(list),
                    Some(one) => Self::from_json(one).into_iter().collect(),
                    None => Vec::new(),
                };
                let branches = object
                    .get("d")
                    .and_then(|d| d.get("b"))
                    .and_then(Value::as_object)
                    .map(|b| {
                        b.iter()
                            .map(|(name, content)| {
                                let nodes = match content {
                                    Value::Array(list) => Self::from_json_children(list),
                                    one => Self::from_json(one).into_iter().collect(),
                                };
                                (SharedString::new(name), nodes.into_boxed_slice())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(RichNode::Element {
                    index: object.get("i").and_then(Value::as_u64).unwrap_or(0) as u32,
                    tag: object.get("t").and_then(Value::as_str).unwrap_or_default().into(),
                    children: children.into_boxed_slice(),
                    branches,
                })
            }
            _ => None,
        }
    }
}

/// One `t_rich!` call site: the key and the English children, parsed on first use.
pub struct RichMessage {
    key: u64,
    source_json: &'static str,
    source: OnceLock<Arc<[RichNode]>>,
}

impl RichMessage {
    /// Called by `t_rich!` with values it computed at compile time.
    pub const fn new(key: u64, source_json: &'static str) -> Self {
        Self {
            key,
            source_json,
            source: OnceLock::new(),
        }
    }

    fn source(&self) -> &Arc<[RichNode]> {
        self.source.get_or_init(|| match serde_json::from_str::<Value>(self.source_json) {
            Ok(Value::Array(list)) => RichNode::from_json_children(&list).into(),
            Ok(one) => RichNode::from_json_children(&[one]).into(),
            Err(_) => Arc::new([]),
        })
    }
}

/// Text rendered from a `<T>` block, in reading order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichText {
    pub segments: Vec<RichSegment>,
}

/// A run of text and the innermost element it sits in (`code`, `strong`, `a`, a
/// component name…). An empty `text` with a tag is a childless element such as an icon.
#[derive(Clone, Debug, PartialEq)]
pub struct RichSegment {
    pub text: SharedString,
    pub tag: Option<SharedString>,
}

impl RichText {
    /// The text with styling dropped.
    pub fn plain(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }

    /// One inline run for `StyledText::new(text).with_highlights(ranges)`, styling
    /// each tagged segment with `style(tag)`. Childless elements are dropped.
    pub fn styled(&self, style: impl Fn(&str) -> Option<HighlightStyle>) -> StyledText {
        let mut text = String::new();
        let mut highlights = Vec::new();
        for segment in &self.segments {
            let start = text.len();
            text.push_str(&segment.text);
            if let Some(highlight) = segment.tag.as_deref().and_then(&style)
                && start < text.len()
            {
                highlights.push((start..text.len(), highlight));
            }
        }
        StyledText::new(text).with_highlights(highlights)
    }
}

/// Resolves a `t_rich!` call: the active catalog's tree, else the English source.
pub fn translate_rich(
    message: &'static RichMessage,
    vars: &[(&'static str, Arg<'_>)],
    branches: &[(u32, Arg<'_>)],
) -> RichText {
    let (_, catalog) = active();
    let mut out = RichText::default();
    let renderer = Renderer { vars, branches };
    match catalog.as_ref().and_then(|c| c.get(message.key)) {
        Some(CatalogValue::Rich(nodes)) => renderer.nodes(nodes, None, &mut out),
        Some(CatalogValue::Text(entry)) => out.segments.push(RichSegment {
            text: entry.text.clone(),
            tag: None,
        }),
        None => renderer.nodes(message.source(), None, &mut out),
    }
    out
}

struct Renderer<'a, 'b> {
    vars: &'a [(&'static str, Arg<'b>)],
    branches: &'a [(u32, Arg<'b>)],
}

impl Renderer<'_, '_> {
    fn nodes(&self, nodes: &[RichNode], tag: Option<&SharedString>, out: &mut RichText) {
        for node in nodes {
            match node {
                RichNode::Text(text) => push(out, text.clone(), tag),
                RichNode::Var(key) => {
                    let value = self.vars.iter().find(|(k, _)| *k == key.as_str()).map(|(_, v)| *v);
                    let text = match value {
                        Some(Arg::Str(s)) => SharedString::new(s),
                        Some(Arg::Num(n)) => js_number_string(n).into(),
                        None => SharedString::default(),
                    };
                    push(out, text, tag);
                }
                RichNode::Element {
                    index,
                    tag: element,
                    children,
                    branches,
                } => {
                    if !branches.is_empty() {
                        // `<Branch>` renders the chosen content with no wrapper of its own.
                        let chosen = self
                            .branches
                            .iter()
                            .find(|(i, _)| i == index)
                            .and_then(|(_, arg)| match arg {
                                Arg::Str(s) => branches.iter().find(|(name, _)| name.as_str() == *s),
                                Arg::Num(_) => None,
                            })
                            .map(|(_, nodes)| &**nodes);
                        self.nodes(chosen.unwrap_or(children), tag, out);
                    } else if children.is_empty() {
                        out.segments.push(RichSegment {
                            text: SharedString::default(),
                            tag: Some(element.clone()),
                        });
                    } else {
                        self.nodes(children, Some(element), out);
                    }
                }
            }
        }
    }
}

/// Appends text, merging it into the previous segment when the style matches.
fn push(out: &mut RichText, text: SharedString, tag: Option<&SharedString>) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.segments.last_mut()
        && last.tag.as_ref() == tag
        && !last.text.is_empty()
    {
        last.text = format!("{}{}", last.text, text).into();
        return;
    }
    out.segments.push(RichSegment {
        text,
        tag: tag.cloned(),
    });
}
