//! Compile-time half of T3 Code's translations. `t!` and `t_rich!` hash their English
//! source into the catalog key gt-react uses, check the ICU syntax and named values,
//! and warn when the key is in no catalog. The runtime lives in `t3code_web::i18n`.

mod key;
mod rich;

#[path = "../../../apps/web/src/i18n/icu.rs"]
#[allow(dead_code)]
mod icu;

use std::{collections::HashMap, path::Path, sync::OnceLock};

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    Expr, Ident, LitStr, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

use key::{DataFormat, jsx_key, parse_key, text_key};

/// Translates an English string into the active UI language. Returns a
/// `SharedString`; a lookup with no values to interpolate does not allocate.
///
/// ```ignore
/// t!("Open settings")
/// t!("Open", context = "verb, opens a file")
/// t!("{count, plural, one {{count} thread} other {{count} threads}}", count = n)
/// ```
///
/// The source must not change by one character: it is the catalog key. Every
/// `{name}` in the source needs a `name = value`, and every value must be used.
#[proc_macro]
pub fn t(input: TokenStream) -> TokenStream {
    let call = parse_macro_input!(input as Call);
    expand_t(call).unwrap_or_else(|e| e.to_compile_error()).into()
}

/// Translates a `<T>` block and returns styled segments. See the markup rules in
/// `rich.rs`: `t_rich!("Clone {repo} and bring the local files git does not track.", repo = name)`.
#[proc_macro]
pub fn t_rich(input: TokenStream) -> TokenStream {
    let call = parse_macro_input!(input as Call);
    expand_rich(call).unwrap_or_else(|e| e.to_compile_error()).into()
}

/// The deflated non-English catalogs as `&[(&str locale, &[u8] web, &[u8] desktop)]`.
#[proc_macro]
pub fn embedded_catalogs(_input: TokenStream) -> TokenStream {
    let out = Path::new(env!("OUT_DIR")).join("i18n");
    let rows = LOCALES.iter().map(|locale| {
        let web = out.join("web").join(format!("{locale}.json.deflate"));
        let desktop = out.join("desktop").join(format!("{locale}.json.deflate"));
        let (web, desktop) = (web.to_str().unwrap(), desktop.to_str().unwrap());
        quote! { (#locale, include_bytes!(#web).as_slice(), include_bytes!(#desktop).as_slice()) }
    });
    quote! { &[#(#rows),*] }.into()
}

const LOCALES: [&str; 9] = ["de", "es", "fr", "ja", "ko", "pt-BR", "ru", "sv", "zh"];

struct Call {
    source: LitStr,
    context: Option<LitStr>,
    args: Vec<(Ident, Expr)>,
}

impl Parse for Call {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let source: LitStr = input.parse()?;
        let mut context = None;
        let mut args = Vec::new();
        if input.parse::<Option<Token![,]>>()?.is_some() {
            let pairs = Punctuated::<(Ident, Expr), Token![,]>::parse_terminated_with(input, |input| {
                let name: Ident = input.parse()?;
                input.parse::<Token![=]>()?;
                Ok((name, input.parse()?))
            })?;
            for (name, value) in pairs {
                if name == "context" {
                    let Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(lit), .. }) = value else {
                        return Err(syn::Error::new(name.span(), "context must be a string literal"));
                    };
                    context = Some(lit);
                } else {
                    args.push((name, value));
                }
            }
        }
        Ok(Call {
            source,
            context,
            args,
        })
    }
}

/// What the English catalogs hold for a key.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    Text,
    Rich,
}

/// Keys of the English web and desktop catalogs, read once per compiler run.
fn catalog() -> &'static HashMap<u64, Entry> {
    static CATALOG: OnceLock<HashMap<u64, Entry>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/assets/i18n");
        let mut keys = HashMap::new();
        for app in ["web", "desktop"] {
            let Ok(text) = std::fs::read_to_string(root.join(app).join("en.json")) else {
                continue;
            };
            let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text) else {
                continue;
            };
            for (hex, value) in map {
                if let Some(key) = parse_key(&hex) {
                    let entry = if value.is_string() { Entry::Text } else { Entry::Rich };
                    keys.entry(key).or_insert(entry);
                }
            }
        }
        keys
    })
}

/// A `deprecated` use is the one stable way for a proc macro to print a warning.
fn missing_key_warning(source: &str, span: Span) -> TokenStream2 {
    let note = format!("i18n: no catalog key for {source:?}; it renders in English in every language");
    let marker = format_ident!("MissingTranslation", span = span);
    quote! {
        #[deprecated(note = #note)]
        #[allow(dead_code)]
        struct #marker;
        let _ = #marker;
    }
}

fn expand_t(call: Call) -> syn::Result<TokenStream2> {
    let source = call.source.value();
    let context = call.context.as_ref().map(LitStr::value);
    let span = call.source.span();

    let icu_key = text_key(&source, context.as_deref(), DataFormat::Icu);
    let jsx_key = text_key(&source, context.as_deref(), DataFormat::Jsx);
    let catalog = catalog();
    let in_catalog = |k| catalog.get(&k) == Some(&Entry::Text);
    // A plain `<T>` child is keyed as JSX and never ICU-formatted. Prefer ICU.
    let (key, format) = if !in_catalog(icu_key) && call.args.is_empty() && in_catalog(jsx_key) {
        (jsx_key, DataFormat::Jsx)
    } else {
        (icu_key, DataFormat::Icu)
    };

    let kind = match format {
        DataFormat::Jsx => quote!(Jsx),
        DataFormat::Icu => {
            let nodes = icu::parse(&source).map_err(|e| {
                syn::Error::new(span, format!("invalid ICU message ({} at byte {})", e.code, e.offset))
            })?;
            let mut used = Vec::new();
            icu::argument_names(&nodes, &mut used);
            for (name, _) in &used {
                if !call.args.iter().any(|(n, _)| n == name) {
                    return Err(syn::Error::new(span, format!("missing value `{name} = …`")));
                }
            }
            for (name, _) in &call.args {
                if !used.iter().any(|(n, _)| name == n) {
                    return Err(syn::Error::new(name.span(), format!("`{{{name}}}` is not in the source")));
                }
            }
            if icu::is_verbatim(&source) { quote!(IcuVerbatim) } else { quote!(Icu) }
        }
    };

    let warning = (!in_catalog(key)).then(|| missing_key_warning(&source, span));
    let args = call.args.iter().map(|(name, value)| {
        let name = name.to_string();
        quote! { (#name, ::t3code_web::i18n::IntoArg::into_arg(&(#value))) }
    });
    Ok(quote! {{
        static MESSAGE: ::t3code_web::i18n::Message = ::t3code_web::i18n::Message {
            key: #key,
            source: #source,
            kind: ::t3code_web::i18n::MessageKind::#kind,
        };
        #warning
        ::t3code_web::i18n::translate(&MESSAGE, &[#(#args),*])
    }})
}

fn expand_rich(call: Call) -> syn::Result<TokenStream2> {
    let markup = call.source.value();
    let span = call.source.span();
    let context = call.context.as_ref().map(LitStr::value);
    let children = rich::parse(&markup).map_err(|e| syn::Error::new(span, format!("invalid t_rich! markup: {e}")))?;
    let key = jsx_key(&rich::sanitized_json(&children), context.as_deref());
    let source_json = rich::catalog_json(&children);

    let (mut vars, mut branches) = (Vec::new(), Vec::new());
    rich::arguments(&children, &mut vars, &mut branches);
    let find = |name: &str| {
        call.args
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
            .ok_or_else(|| syn::Error::new(span, format!("missing value `{name} = …`")))
    };
    let var_args = vars
        .iter()
        .map(|(name, index)| {
            let value = find(name)?;
            let k = format!("_gt_value_{index}");
            Ok(quote! { (#k, ::t3code_web::i18n::IntoArg::into_arg(&(#value))) })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let branch_args = branches
        .iter()
        .map(|(name, index)| {
            let value = find(name)?;
            Ok(quote! { (#index, ::t3code_web::i18n::IntoArg::into_arg(&(#value))) })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    for (name, _) in &call.args {
        if !vars.iter().chain(&branches).any(|(n, _)| name == n) {
            return Err(syn::Error::new(name.span(), format!("`{name}` is not in the markup")));
        }
    }

    let warning = (catalog().get(&key) != Some(&Entry::Rich) && catalog().get(&key) != Some(&Entry::Text))
        .then(|| missing_key_warning(&markup, span));
    Ok(quote! {{
        static MESSAGE: ::t3code_web::i18n::RichMessage = ::t3code_web::i18n::RichMessage::new(#key, #source_json);
        #warning
        ::t3code_web::i18n::translate_rich(&MESSAGE, &[#(#var_args),*], &[#(#branch_args),*])
    }})
}
