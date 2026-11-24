use std::borrow::Cow;
use std::sync::Arc;

use schemars::{JsonSchema, json_schema};

/// The private tag [`FontFeatures::with_relative_size`] stores its scale under, in thousandths.
const RELATIVE_SIZE_TAG: &str = "T3SZ";

/// The OpenType features that can be configured for a given font.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct FontFeatures(pub Arc<Vec<(String, u32)>>);

impl Default for FontFeatures {
    /// Shares one empty list, since a default `TextStyle` is built many times per frame.
    fn default() -> Self {
        static EMPTY: std::sync::LazyLock<Arc<Vec<(String, u32)>>> =
            std::sync::LazyLock::new(|| Arc::new(Vec::new()));
        Self(EMPTY.clone())
    }
}

impl FontFeatures {
    /// Disables `calt`.
    pub fn disable_ligatures() -> Self {
        Self(Arc::new(vec![("calt".into(), 0)]))
    }

    /// Get the tag name list of the font OpenType features
    /// only enabled or disabled features are returned
    pub fn tag_value_list(&self) -> &[(String, u32)] {
        self.0.as_slice()
    }

    /// These features plus a size for the run of `scale` times its line's font size, the way
    /// CSS `font-size: 0.85em` sizes inline code inside a sentence. Only the macOS text system
    /// applies it; elsewhere the run keeps the line's size. It is carried as a private feature
    /// tag, which shapers ignore.
    pub fn with_relative_size(&self, scale: f32) -> Self {
        let mut list: Vec<_> = self.0.iter().filter(|(tag, _)| tag != RELATIVE_SIZE_TAG).cloned().collect();
        list.push((RELATIVE_SIZE_TAG.into(), (scale * 1000.).round() as u32));
        Self(Arc::new(list))
    }

    /// The scale [`Self::with_relative_size`] set, if any.
    pub fn relative_size(&self) -> Option<f32> {
        self.0
            .iter()
            .find(|(tag, _)| tag == RELATIVE_SIZE_TAG)
            .map(|(_, value)| *value as f32 / 1000.)
    }

    /// These features without a relative size.
    pub fn without_relative_size(&self) -> Self {
        Self(Arc::new(self.0.iter().filter(|(tag, _)| tag != RELATIVE_SIZE_TAG).cloned().collect()))
    }

    /// Returns whether the `calt` feature is enabled.
    ///
    /// Returns `None` if the feature is not present.
    pub fn is_calt_enabled(&self) -> Option<bool> {
        self.0
            .iter()
            .find(|(feature, _)| feature == "calt")
            .map(|(_, value)| *value == 1)
    }
}

impl std::fmt::Debug for FontFeatures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("FontFeatures");
        for (tag, value) in self.tag_value_list() {
            debug.field(tag, value);
        }

        debug.finish()
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum FeatureValue {
    Bool(bool),
    Number(serde_json::Number),
}

impl<'de> serde::Deserialize<'de> for FontFeatures {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{MapAccess, Visitor};
        use std::fmt;

        struct FontFeaturesVisitor;

        impl<'de> Visitor<'de> for FontFeaturesVisitor {
            type Value = FontFeatures;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of font features")
            }

            fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut feature_list = Vec::new();

                while let Some((key, value)) =
                    access.next_entry::<String, Option<FeatureValue>>()?
                {
                    if !is_valid_feature_tag(&key) {
                        log::error!("Incorrect font feature tag: {}", key);
                        continue;
                    }
                    if let Some(value) = value {
                        match value {
                            FeatureValue::Bool(enable) => {
                                if enable {
                                    feature_list.push((key, 1));
                                } else {
                                    feature_list.push((key, 0));
                                }
                            }
                            FeatureValue::Number(value) => {
                                if value.is_u64() {
                                    feature_list.push((key, value.as_u64().unwrap() as u32));
                                } else {
                                    log::error!(
                                        "Incorrect font feature value {} for feature tag {}",
                                        value,
                                        key
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                }

                Ok(FontFeatures(Arc::new(feature_list)))
            }
        }

        let features = deserializer.deserialize_map(FontFeaturesVisitor)?;
        Ok(features)
    }
}

impl serde::Serialize for FontFeatures {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(None)?;

        for (tag, value) in self.tag_value_list() {
            map.serialize_entry(tag, value)?;
        }

        map.end()
    }
}

impl JsonSchema for FontFeatures {
    fn schema_name() -> Cow<'static, str> {
        "FontFeatures".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        json_schema!({
            "type": "object",
            "patternProperties": {
                "[0-9a-zA-Z]{4}$": {
                    "type": ["boolean", "integer"],
                    "minimum": 0,
                    "multipleOf": 1
                }
            },
            "additionalProperties": false
        })
    }
}

fn is_valid_feature_tag(tag: &str) -> bool {
    tag.len() == 4 && tag.chars().all(|c| c.is_ascii_alphanumeric())
}
