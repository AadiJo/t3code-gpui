//! The embedded translation catalogs. The build deflates each non-English JSON file
//! from `apps/web/assets/i18n`; a locale is inflated and parsed only when it
//! becomes active, and its memory is freed when another one replaces it.

use std::{
    collections::HashMap,
    fmt,
    hash::{BuildHasherDefault, Hasher},
    io::Read,
};

use flate2::read::DeflateDecoder;
use gpui_kit::SharedString;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};

use super::{icu, locales::Locale, rich::RichNode};

/// `(locale, deflated web catalog, deflated desktop catalog)` for every non-English locale.
static EMBEDDED: &[(&str, &[u8], &[u8])] = t3code_i18n_macros::embedded_catalogs!();

/// One locale's translations, keyed by the `u64` form of gt's 16-hex-digit hash.
pub struct Catalog {
    entries: HashMap<u64, CatalogValue, BuildHasherDefault<KeyHasher>>,
}

/// A translated string, or a `<T>` block's children.
pub enum CatalogValue {
    Text(TextEntry),
    Rich(std::sync::Arc<[RichNode]>),
}

/// A translated string and whether it prints as-is with no values.
pub struct TextEntry {
    pub text: SharedString,
    pub verbatim: bool,
}

impl Catalog {
    /// Looks up a key.
    pub fn get(&self, key: u64) -> Option<&CatalogValue> {
        self.entries.get(&key)
    }

    /// The number of keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the catalog holds no keys.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Inflates and parses a locale's web and desktop catalogs into one map; web
    /// wins when both have a key. English has no catalog and returns `None`.
    pub fn load(locale: Locale) -> Option<Catalog> {
        let (_, web, desktop) = EMBEDDED.iter().find(|(tag, ..)| *tag == locale.tag())?;
        let mut entries = HashMap::default();
        for blob in [desktop, web] {
            let mut json = Vec::with_capacity(blob.len() * 4);
            DeflateDecoder::new(*blob).read_to_end(&mut json).ok()?;
            let mut deserializer = serde_json::Deserializer::from_slice(&json);
            deserializer
                .deserialize_map(EntriesVisitor(&mut entries))
                .ok()?;
        }
        entries.shrink_to_fit();
        Some(Catalog { entries })
    }

    /// Total size of the embedded, deflated catalogs in bytes.
    pub fn embedded_size() -> usize {
        EMBEDDED.iter().map(|(_, w, d)| w.len() + d.len()).sum()
    }
}

/// Keys are already uniform hashes, so the map uses them as-is.
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 << 8) | u64::from(b);
        }
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
}

type Entries = HashMap<u64, CatalogValue, BuildHasherDefault<KeyHasher>>;

struct EntriesVisitor<'a>(&'a mut Entries);

impl<'de> Visitor<'de> for EntriesVisitor<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a translation catalog object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(hex) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            let key = u64::from_str_radix(&hex, 16).map_err(de::Error::custom)?;
            let value = map.next_value_seed(ValueSeed)?;
            self.0.insert(key, value);
        }
        Ok(())
    }
}

struct ValueSeed;

impl<'de> de::DeserializeSeed<'de> for ValueSeed {
    type Value = CatalogValue;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<CatalogValue, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ValueSeed {
    type Value = CatalogValue;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a string or a <T> children array")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<CatalogValue, E> {
        Ok(CatalogValue::Text(TextEntry {
            verbatim: icu::is_verbatim(text),
            text: SharedString::new(text),
        }))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<CatalogValue, A::Error> {
        let mut children = Vec::new();
        while let Some(child) = seq.next_element::<serde_json::Value>()? {
            children.push(child);
        }
        Ok(CatalogValue::Rich(RichNode::from_json_children(&children).into()))
    }

    /// A `<T>` block with a single element or variable child.
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<CatalogValue, A::Error> {
        use serde::Deserialize;
        let child = serde_json::Value::deserialize(de::value::MapAccessDeserializer::new(map))?;
        Ok(CatalogValue::Rich(RichNode::from_json_children(&[child]).into()))
    }
}
