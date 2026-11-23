//! Helpers the generated wire types are built from: union dispatch over raw JSON, branded id
//! newtypes, string enums, literal markers and forward-compatible fields.

use serde::de::{self, DeserializeOwned, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;
use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;

/// The JSON kind of a value, read from its first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonKind {
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

/// A discriminator value read from a union's raw JSON. A missing key and `null` are both `Absent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag<'a> {
    Str(Cow<'a, str>),
    Bool(bool),
    Absent,
    Other,
}

/// A union value held as raw JSON text while its decoder picks a branch. `E` is the error type of
/// the deserializer it came from, so generated decoders can use `?` throughout.
///
/// Decoding a branch re-parses the text, which costs one extra scan per union level but never
/// builds an intermediate tree. Needs a serde_json deserializer (text or `Value`) underneath.
pub struct Raw<E> {
    raw: Box<RawValue>,
    _error: PhantomData<E>,
}

impl<E: de::Error> Raw<E> {
    /// Captures the next value of `d` as raw JSON.
    pub fn new<'de, D: Deserializer<'de, Error = E>>(d: D) -> Result<Self, E> {
        Ok(Self { raw: Box::<RawValue>::deserialize(d)?, _error: PhantomData })
    }

    /// Decodes the whole value as `T`.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, E> {
        serde_json::from_str(self.raw.get()).map_err(E::custom)
    }

    /// The JSON kind of the value.
    pub fn kind(&self) -> JsonKind {
        match self.raw.get().trim_start().as_bytes().first() {
            Some(b'{') => JsonKind::Object,
            Some(b'[') => JsonKind::Array,
            Some(b'"') => JsonKind::String,
            Some(b't' | b'f') => JsonKind::Bool,
            Some(b'n') => JsonKind::Null,
            _ => JsonKind::Number,
        }
    }

    /// The value of `key` when the value is an object.
    pub fn tag(&self, key: &str) -> Result<Tag<'_>, E> {
        if self.kind() != JsonKind::Object {
            return Ok(Tag::Other);
        }
        let mut de = serde_json::Deserializer::from_str(self.raw.get());
        de.deserialize_map(TagVisitor { key }).map_err(E::custom)
    }

    /// Whether the value is an object that has `key`.
    pub fn has_key(&self, key: &str) -> Result<bool, E> {
        if self.kind() != JsonKind::Object {
            return Ok(false);
        }
        let mut de = serde_json::Deserializer::from_str(self.raw.get());
        de.deserialize_map(HasKeyVisitor { key }).map_err(E::custom)
    }

    /// The error for a value no branch of union `ty` accepts.
    pub fn unexpected(&self, kind: JsonKind, ty: &str) -> E {
        E::custom(format_args!("unexpected {kind:?} for {ty}"))
    }
}

/// A map key, borrowed from the input when it has no escapes.
struct Key<'de>(Cow<'de, str>);

impl<'de> Deserialize<'de> for Key<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Key<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E> {
                Ok(Key(Cow::Borrowed(v)))
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(Key(Cow::Owned(v.to_owned())))
            }
            fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
                Ok(Key(Cow::Owned(v)))
            }
        }
        d.deserialize_str(V)
    }
}

/// The discriminator's value: a string, a bool, null, or anything else (skipped).
struct TagValue<'de>(Tag<'de>);

impl<'de> Deserialize<'de> for TagValue<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = TagValue<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E> {
                Ok(TagValue(Tag::Str(Cow::Borrowed(v))))
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(TagValue(Tag::Str(Cow::Owned(v.to_owned()))))
            }
            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
                Ok(TagValue(Tag::Bool(v)))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(TagValue(Tag::Absent))
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(TagValue(Tag::Absent))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(TagValue(Tag::Other))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(TagValue(Tag::Other))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(TagValue(Tag::Other))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(TagValue(Tag::Other))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(TagValue(Tag::Other))
            }
        }
        d.deserialize_any(V)
    }
}

struct TagVisitor<'k> {
    key: &'k str,
}

impl<'de> Visitor<'de> for TagVisitor<'_> {
    type Value = Tag<'de>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut found = Tag::Absent;
        let mut seen = false;
        while let Some(Key(k)) = map.next_key()? {
            if !seen && k == self.key {
                found = map.next_value::<TagValue>()?.0;
                seen = true;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

struct HasKeyVisitor<'k> {
    key: &'k str,
}

impl<'de> Visitor<'de> for HasKeyVisitor<'_> {
    type Value = bool;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
        let mut found = false;
        while let Some(Key(k)) = map.next_key()? {
            found |= k == self.key;
            map.next_value::<IgnoredAny>()?;
        }
        Ok(found)
    }
}

/// Decodes a forward-compatible field: a value this client cannot parse becomes `None` instead
/// of failing the whole message, as the TypeScript contract does.
pub fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = Option::<Box<RawValue>>::deserialize(d)?;
    Ok(raw.and_then(|r| serde_json::from_str(r.get()).ok()))
}

/// An object with no declared fields (`Schema.Struct({})`). Writes `{}`; accepts any value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Empty;

impl Serialize for Empty {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        s.serialize_map(Some(0))?.end()
    }
}

impl<'de> Deserialize<'de> for Empty {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        IgnoredAny::deserialize(d).map(|_| Empty)
    }
}

/// The error type of an RPC or endpoint that cannot fail. No value decodes into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Never {}

impl Serialize for Never {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        match *self {}
    }
}

impl<'de> Deserialize<'de> for Never {
    fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(de::Error::custom("this call declares no error"))
    }
}

macro_rules! bool_literal {
    ($(#[$m:meta])* $name:ident = $value:literal) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_bool($value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                if bool::deserialize(d)? == $value {
                    Ok($name)
                } else {
                    Err(de::Error::custom(concat!("expected ", stringify!($value))))
                }
            }
        }
    };
}

bool_literal!(
    /// The literal `true` (e.g. the `idTable: true` marker).
    True = true
);
bool_literal!(
    /// The literal `false`.
    False = false
);

/// Decodes a string through `parse`, reporting `expected` when it returns `None`.
pub fn deserialize_str_enum<'de, D, T>(
    d: D,
    parse: fn(&str) -> Option<T>,
    expected: &'static [&'static str],
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
{
    struct V<T> {
        parse: fn(&str) -> Option<T>,
        expected: &'static [&'static str],
    }
    impl<T> Visitor<'_> for V<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "one of {:?}", self.expected)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
            (self.parse)(v).ok_or_else(|| E::unknown_variant(v, self.expected))
        }
    }
    d.deserialize_str(V { parse, expected })
}

/// Declares a branded id: a cheap-to-clone `Arc<str>` newtype that is its own type per brand.
macro_rules! id_newtype {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub ::std::sync::Arc<str>);

        impl $name {
            /// Wraps a raw id.
            pub fn new(id: impl Into<::std::sync::Arc<str>>) -> Self {
                Self(id.into())
            }

            /// The id as written on the wire.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl ::std::convert::AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl ::std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl ::std::convert::From<&str> for $name {
            fn from(id: &str) -> Self {
                Self(id.into())
            }
        }

        impl ::std::convert::From<String> for $name {
            fn from(id: String) -> Self {
                Self(id.into())
            }
        }
    };
}
pub(crate) use id_newtype;

/// Declares a closed string enum with `as_str`, `from_wire` and `ALL`.
macro_rules! string_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident = $value:literal,)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($variant,)*
        }

        impl $name {
            /// Every value, in schema order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            /// The value as written on the wire.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)*
                }
            }

            /// Parses a wire value.
            pub fn from_wire(s: &str) -> Option<Self> {
                match s {
                    $($value => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                $crate::wire::deserialize_str_enum(d, Self::from_wire, &[$($value,)*])
            }
        }
    };
}
pub(crate) use string_enum;
