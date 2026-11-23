//! Translations and locale-aware formatting. Ports `apps/web/src/i18n`.
//!
//! Wrap user-facing English in `t!` (strings, `msg()`/`gt()` in the TypeScript) or
//! `t_rich!` (`<T>` blocks). Both hash the source at compile time into the same key
//! gt-react uses, so the catalogs in `apps/web/assets/i18n` stay a plain copy of the
//! TypeScript app's. A string with no catalog key compiles with a warning and renders
//! English. Call [`runtime::init`] at startup and [`runtime::apply_language_setting`]
//! when the `language` setting changes; views re-render on a switch.

pub mod catalog;
pub mod duration_format;
pub mod icu;
pub mod list_format;
mod locale_data;
pub mod locale_format;
pub mod locales;
pub mod messages;
pub mod number_format;
pub mod plural;
pub mod rich;
pub mod runtime;

pub use locales::Locale;
pub use messages::{Arg, IntoArg, Message, MessageKind, active_locale, translate};
pub use rich::{RichMessage, RichSegment, RichText, translate_rich};
pub use t3code_i18n_macros::{t, t_rich};
