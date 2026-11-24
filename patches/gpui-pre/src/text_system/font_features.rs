use std::borrow::Cow;
use std::sync::Arc;

use schemars::{JsonSchema, json_schema};

/// The private tags [`FontFeatures::with_run_layout`] stores a [`RunLayout`] under, in
/// thousandths: scale, padding in ems, padding in pixels.
const RUN_LAYOUT_TAGS: [&str; 3] = ["T3SZ", "T3PE", "T3PP"];

/// How a run sits inside its line, the way CSS lays out an inline element such as inline
/// code (`font-size: 0.85em; padding-inline: 0.35em; border: 1px`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunLayout {
    /// The run's font size relative to the line's.
    pub scale: f32,
    /// Space before and after the run, in ems of the run's own font size...
    pub padding_em: f32,
    /// ...plus this many pixels.
    pub padding_px: f32,
}

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

    /// These features plus how the run is laid out inside its line (see [`RunLayout`]).
    /// Only the macOS text system applies it; elsewhere the run is laid out as if it were
    /// absent. It is carried as private feature tags, which shapers ignore.
    pub fn with_run_layout(&self, layout: RunLayout) -> Self {
        let mut list: Vec<_> = self.0.iter().filter(|(tag, _)| !RUN_LAYOUT_TAGS.contains(&tag.as_str())).cloned().collect();
        let values = [layout.scale, layout.padding_em, layout.padding_px];
        list.extend(RUN_LAYOUT_TAGS.iter().zip(values).map(|(tag, value)| ((*tag).into(), (value * 1000.).round() as u32)));
        Self(Arc::new(list))
    }

    /// The layout [`Self::with_run_layout`] set, if any.
    pub fn run_layout(&self) -> Option<RunLayout> {
        let value = |tag: &str| self.0.iter().find(|(t, _)| t == tag).map(|(_, value)| *value as f32 / 1000.);
        Some(RunLayout {
            scale: value(RUN_LAYOUT_TAGS[0])?,
            padding_em: value(RUN_LAYOUT_TAGS[1])?,
            padding_px: value(RUN_LAYOUT_TAGS[2])?,
        })
    }

    /// These features without a run layout.
    pub fn without_run_layout(&self) -> Self {
        Self(Arc::new(self.0.iter().filter(|(tag, _)| !RUN_LAYOUT_TAGS.contains(&tag.as_str())).cloned().collect()))
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
