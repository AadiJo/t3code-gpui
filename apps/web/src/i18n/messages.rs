//! Call-time translation, the runtime half of `t!`. Ports `apps/web/src/i18n/messages.ts`:
//! look the key up in the active catalog, format it as ICU in that locale, and fall
//! back to the English source (with English plural rules) when it is missing.

use std::{
    borrow::Cow,
    sync::{Arc, RwLock},
};

use gpui_kit::SharedString;

use super::{
    catalog::{Catalog, CatalogValue},
    icu::{self, Node},
    locale_format::{format_decimal, locale_data},
    locales::Locale,
    plural::plural_category,
};

/// One `t!` call site: the catalog key and English source, computed at compile time.
pub struct Message {
    pub key: u64,
    pub source: &'static str,
    pub kind: MessageKind,
}

/// How to treat a catalog value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// An ICU message from `msg()`/`gt()`.
    Icu,
    /// An ICU message whose English is plain text, so English needs no formatting.
    IcuVerbatim,
    /// A plain-text `<T>` child: printed as-is, never ICU-formatted.
    Jsx,
}

/// A value interpolated into a message.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    Str(&'a str),
    Num(f64),
}

/// Converts a `t!` value into an [`Arg`]. Numbers stay numbers so plurals select
/// on them; `{count}` still prints them unformatted, as gt does.
pub trait IntoArg {
    fn into_arg(&self) -> Arg<'_>;
}

impl<T: IntoArg + ?Sized> IntoArg for &T {
    fn into_arg(&self) -> Arg<'_> {
        (**self).into_arg()
    }
}

macro_rules! str_args {
    ($($ty:ty),*) => {$(
        impl IntoArg for $ty {
            fn into_arg(&self) -> Arg<'_> {
                Arg::Str(self.as_ref())
            }
        }
    )*};
}
str_args!(str, String, SharedString, Arc<str>, Cow<'_, str>);

macro_rules! num_args {
    ($($ty:ty),*) => {$(
        impl IntoArg for $ty {
            fn into_arg(&self) -> Arg<'_> {
                Arg::Num(*self as f64)
            }
        }
    )*};
}
num_args!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64);

struct Active {
    locale: Locale,
    catalog: Option<Arc<Catalog>>,
}

static ACTIVE: RwLock<Active> = RwLock::new(Active {
    locale: Locale::En,
    catalog: None,
});

/// The UI language `t!` resolves in right now.
pub fn active_locale() -> Locale {
    ACTIVE.read().unwrap().locale
}

/// Swaps the active catalog. The previous one is dropped once no lookup holds it.
pub(crate) fn set_active(locale: Locale, catalog: Option<Arc<Catalog>>) {
    *ACTIVE.write().unwrap() = Active { locale, catalog };
}

/// The active locale and catalog, for the rich-text renderer.
pub(crate) fn active() -> (Locale, Option<Arc<Catalog>>) {
    let active = ACTIVE.read().unwrap();
    (active.locale, active.catalog.clone())
}

/// Resolves a `t!` call. Without values this is one hash-map probe and a
/// reference-count bump (or a static string for English).
pub fn translate(message: &'static Message, args: &[(&'static str, Arg<'_>)]) -> SharedString {
    {
        let active = ACTIVE.read().unwrap();
        let entry = active.catalog.as_ref().and_then(|c| c.get(message.key));
        if let Some(CatalogValue::Text(entry)) = entry {
            if message.kind == MessageKind::Jsx || (args.is_empty() && entry.verbatim) {
                return entry.text.clone();
            }
            if let Ok(text) = format_message(&entry.text, args, active.locale) {
                return text.into();
            }
            // gt retries the English source, still in the target locale.
            return format_message(message.source, args, active.locale)
                .unwrap_or_else(|_| message.source.to_owned())
                .into();
        }
    }
    if message.kind != MessageKind::Icu && args.is_empty() {
        return SharedString::new_static(message.source);
    }
    // Untranslated text is English, so it takes English plural rules.
    format_message(message.source, args, Locale::En)
        .unwrap_or_else(|_| message.source.to_owned())
        .into()
}

/// Why a message could not be formatted; the caller falls back to the source.
#[derive(Debug)]
pub enum FormatError {
    Parse(icu::ParseError),
    MissingArg(String),
}

/// Formats an ICU message in `locale`, the way `gtFallback` does.
pub fn format_message(message: &str, args: &[(&str, Arg<'_>)], locale: Locale) -> Result<String, FormatError> {
    let nodes = icu::parse(message).map_err(FormatError::Parse)?;
    let mut out = String::with_capacity(message.len() + 16);
    write_nodes(&nodes, args, locale, None, &mut out)?;
    Ok(out)
}

fn find_arg<'a>(args: &[(&str, Arg<'a>)], name: &str) -> Result<Arg<'a>, FormatError> {
    args.iter()
        .find(|(n, _)| *n == name)
        .map(|(_, a)| *a)
        .ok_or_else(|| FormatError::MissingArg(name.to_owned()))
}

fn write_nodes(
    nodes: &[Node<'_>],
    args: &[(&str, Arg<'_>)],
    locale: Locale,
    plural_value: Option<f64>,
    out: &mut String,
) -> Result<(), FormatError> {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Pound => {
                if let Some(value) = plural_value {
                    out.push_str(&format_decimal(value, &locale_data(locale.tag()).number, 0, 3));
                }
            }
            Node::Arg(name) => push_arg(out, find_arg(args, name)?),
            Node::Select { arg, options } => {
                let value = arg_string(find_arg(args, arg)?);
                let branch = options
                    .iter()
                    .find(|(s, _)| **s == *value)
                    .or_else(|| options.iter().find(|(s, _)| *s == "other"))
                    .map(|(_, b)| b);
                if let Some(branch) = branch {
                    write_nodes(branch, args, locale, None, out)?;
                }
            }
            Node::Plural { arg, offset, options } => {
                let raw = find_arg(args, arg)?;
                let exact = format!("={}", arg_string(raw));
                let value = match raw {
                    Arg::Num(n) => n,
                    Arg::Str(s) => js_number(s),
                } - offset;
                let has_category = options.iter().any(|(s, _)| *s != "other" && !s.starts_with('='));
                let branch = options
                    .iter()
                    .find(|(s, _)| *s == exact)
                    .or_else(|| {
                        has_category.then(|| plural_category(locale, value).as_str()).and_then(|category| {
                            options.iter().find(|(s, _)| *s == category)
                        })
                    })
                    .or_else(|| options.iter().find(|(s, _)| *s == "other"))
                    .map(|(_, b)| b);
                if let Some(branch) = branch {
                    write_nodes(branch, args, locale, Some(value), out)?;
                }
            }
        }
    }
    Ok(())
}

fn push_arg(out: &mut String, arg: Arg<'_>) {
    match arg {
        Arg::Str(s) => out.push_str(s),
        Arg::Num(n) => out.push_str(&js_number_string(n)),
    }
}

fn arg_string(arg: Arg<'_>) -> Cow<'_, str> {
    match arg {
        Arg::Str(s) => Cow::Borrowed(s),
        Arg::Num(n) => Cow::Owned(js_number_string(n)),
    }
}

/// `String(n)` for a JavaScript number.
pub fn js_number_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if n == 0.0 {
        return "0".into();
    }
    let abs = n.abs();
    if (1e-6..1e21).contains(&abs) {
        return n.to_string();
    }
    // JavaScript switches to exponent form outside that range and always signs it.
    let text = format!("{n:e}");
    match text.split_once('e') {
        Some((mantissa, exp)) if !exp.starts_with('-') => format!("{mantissa}e+{exp}"),
        _ => text,
    }
}

/// `Number(s)` for the strings a plural might receive.
fn js_number(s: &str) -> f64 {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        0.0
    } else {
        trimmed.parse().unwrap_or(f64::NAN)
    }
}
