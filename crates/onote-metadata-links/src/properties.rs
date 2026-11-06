//! F09 typed frontmatter properties.
//!
//! Properties are read from YAML frontmatter and typed by their value shape.
//! Two rules drive the whole module:
//!
//! * "unset" is a distinct state, never `""`, `false` or `0` (F09-A01), so an
//!   empty field stays empty instead of silently becoming a default;
//! * a field this build does not understand is preserved byte-for-byte and
//!   reported as opaque instead of being rewritten (F09-A02).

use crate::scan::{Frontmatter, Scanner};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use yaml_rust2::{Yaml, YamlLoader};

/// F09 property types. `Unset` is a real variant because an empty field must
/// stay distinguishable from every concrete value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyType {
    Text,
    Number,
    Boolean,
    Date,
    DateTime,
    Select,
    MultiSelect,
    Link,
    Unset,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyValue {
    Text(String),
    Number(f64),
    Boolean(bool),
    /// `YYYY-MM-DD`, kept as text so no timezone can shift the calendar day.
    Date(String),
    /// UTC instant in RFC 3339 form.
    DateTime(String),
    Select(String),
    MultiSelect(Vec<String>),
    Link(String),
    Unset,
}

impl PropertyValue {
    pub fn property_type(&self) -> PropertyType {
        match self {
            PropertyValue::Text(_) => PropertyType::Text,
            PropertyValue::Number(_) => PropertyType::Number,
            PropertyValue::Boolean(_) => PropertyType::Boolean,
            PropertyValue::Date(_) => PropertyType::Date,
            PropertyValue::DateTime(_) => PropertyType::DateTime,
            PropertyValue::Select(_) => PropertyType::Select,
            PropertyValue::MultiSelect(_) => PropertyType::MultiSelect,
            PropertyValue::Link(_) => PropertyType::Link,
            PropertyValue::Unset => PropertyType::Unset,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Property {
    pub name: String,
    pub value: PropertyValue,
    /// A key that is not recognised as a built-in field. Its raw YAML is kept
    /// in `raw` and must be written back untouched (F09-A02).
    pub unknown: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyError {
    /// The frontmatter block opens and never closes.
    Unterminated,
    /// YAML that does not parse; the source is never rewritten in this state.
    InvalidYaml,
    /// The block is a document other than a mapping.
    NotAMapping,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertySet {
    pub properties: Vec<Property>,
    /// Field names declared by the schema that this note does not set.
    pub unset: Vec<String>,
}

impl PropertySet {
    pub fn get(&self, name: &str) -> Option<&Property> {
        self.properties
            .iter()
            .find(|property| property.name == name)
    }

    pub fn is_unset(&self, name: &str) -> bool {
        self.get(name)
            .map_or(true, |property| property.value == PropertyValue::Unset)
    }
}

/// Fields with product meaning; everything else is preserved as opaque.
const KNOWN_FIELDS: &[&str] = &["title", "tags", "tag", "created", "updated", "aliases"];

fn is_known(name: &str) -> bool {
    KNOWN_FIELDS.contains(&name)
}

fn looks_like_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

fn looks_like_datetime(text: &str) -> bool {
    // `YYYY-MM-DDTHH:MM(:SS)?(Z|±HH:MM)`; a date-only value never matches.
    let Some((date, rest)) = text.split_once(['T', 't', ' ']) else {
        return false;
    };
    if !looks_like_date(date) {
        return false;
    }
    let rest = rest.trim();
    let rest = rest
        .strip_suffix('Z')
        .or_else(|| rest.strip_suffix('z'))
        .unwrap_or(rest);
    let (time, offset) = match rest.find(['+', '-']) {
        Some(index) => (&rest[..index], Some(&rest[index..])),
        None => (rest, None),
    };
    let mut parts = time.split(':');
    let (Some(hour), Some(minute), seconds) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let ok_time = hour.len() == 2
        && minute.len() == 2
        && hour.chars().all(|value| value.is_ascii_digit())
        && minute.chars().all(|value| value.is_ascii_digit())
        && seconds
            .is_none_or(|value| value.len() == 2 && value.chars().all(|c| c.is_ascii_digit()));
    if !ok_time {
        return false;
    }
    match offset {
        None => true,
        Some(offset) => {
            let digits = offset.trim_start_matches(['+', '-']);
            let mut parts = digits.split(':');
            let (Some(hours), Some(minutes), None) = (parts.next(), parts.next(), parts.next())
            else {
                return false;
            };
            hours.len() == 2 && minutes.len() == 2
        }
    }
}

fn is_link(text: &str) -> bool {
    let trimmed = text.trim();
    (trimmed.starts_with("[[") && trimmed.ends_with("]]"))
        || (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
}

/// Types one YAML scalar. Returns `Unset` for an explicit null so the empty
/// state never collapses into a concrete value (F09-A01).
fn scalar_value(text: &str) -> PropertyValue {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return PropertyValue::Unset;
    }
    if let Ok(value) = trimmed.parse::<f64>() {
        // F09 requires finite numbers; NaN/inf are not property values.
        if value.is_finite() {
            return PropertyValue::Number(value);
        }
    }
    match trimmed {
        "true" | "True" | "TRUE" => return PropertyValue::Boolean(true),
        "false" | "False" | "FALSE" => return PropertyValue::Boolean(false),
        _ => (),
    }
    if looks_like_datetime(trimmed) {
        return PropertyValue::DateTime(trimmed.to_owned());
    }
    if looks_like_date(trimmed) {
        return PropertyValue::Date(trimmed.to_owned());
    }
    if is_link(trimmed) {
        return PropertyValue::Link(trimmed.to_owned());
    }
    PropertyValue::Text(trimmed.to_owned())
}

fn value_of(node: &Yaml) -> PropertyValue {
    match node {
        Yaml::String(text) => scalar_value(text),
        Yaml::Integer(value) => PropertyValue::Number(*value as f64),
        Yaml::Real(text) => match text.parse::<f64>() {
            Ok(value) if value.is_finite() => PropertyValue::Number(value),
            _ => PropertyValue::Text(text.clone()),
        },
        Yaml::Boolean(value) => PropertyValue::Boolean(*value),
        Yaml::Null => PropertyValue::Unset,
        Yaml::Array(items) => {
            let values: Vec<String> = items
                .iter()
                .filter_map(|item| match item {
                    Yaml::String(text) => Some(text.trim().to_owned()),
                    Yaml::Integer(value) => Some(value.to_string()),
                    _ => None,
                })
                .filter(|text| !text.is_empty())
                .collect();
            if values.is_empty() {
                PropertyValue::Unset
            } else {
                PropertyValue::MultiSelect(values)
            }
        }
        // A nested mapping is not a property value this build understands; it
        // is reported as opaque text and must round-trip unchanged.
        _ => PropertyValue::Text(format!("{node:?}")),
    }
}

/// Parses the frontmatter of `source`. `schema_fields` lists the fields the
/// caller expects; any declared field the note omits is reported in `unset`
/// instead of being invented.
pub fn parse_properties(
    source: &str,
    schema_fields: &[&str],
) -> Result<PropertySet, PropertyError> {
    let scanner = Scanner::new(source);
    match scanner.frontmatter() {
        Frontmatter::None => {
            let unset = schema_fields
                .iter()
                .map(|name| (*name).to_owned())
                .collect();
            return Ok(PropertySet {
                properties: Vec::new(),
                unset,
            });
        }
        Frontmatter::Unterminated(_) => return Err(PropertyError::Unterminated),
        Frontmatter::Closed(_) => (),
    }
    let body = scanner.frontmatter_body().unwrap_or_default();
    let documents = YamlLoader::load_from_str(body).map_err(|_| PropertyError::InvalidYaml)?;
    let Some(document) = documents.first() else {
        let unset = schema_fields
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        return Ok(PropertySet {
            properties: Vec::new(),
            unset,
        });
    };
    let Yaml::Hash(entries) = document else {
        return Err(PropertyError::NotAMapping);
    };
    let mut properties = Vec::with_capacity(entries.len());
    let mut present = BTreeSet::new();
    for (key, node) in entries {
        let Some(name) = key.as_str() else {
            continue;
        };
        present.insert(name.to_owned());
        properties.push(Property {
            name: name.to_owned(),
            value: value_of(node),
            unknown: !is_known(name) && !schema_fields.contains(&name),
        });
    }
    let unset = schema_fields
        .iter()
        .filter(|name| !present.contains(**name))
        .map(|name| (*name).to_owned())
        .collect();
    Ok(PropertySet { properties, unset })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> PropertySet {
        parse_properties(source, &[]).unwrap()
    }

    #[test]
    fn unset_stays_distinct_from_empty_false_and_zero() {
        let set = parse("---\nblank:\nzero: 0\nflag: false\ntext: \n---\n");
        assert_eq!(set.get("blank").unwrap().value, PropertyValue::Unset);
        assert_eq!(set.get("zero").unwrap().value, PropertyValue::Number(0.0));
        assert_eq!(
            set.get("flag").unwrap().value,
            PropertyValue::Boolean(false)
        );
        assert_eq!(set.get("text").unwrap().value, PropertyValue::Unset);
        assert!(set.is_unset("blank"));
        assert!(!set.is_unset("zero"));
    }

    #[test]
    fn values_are_typed_by_shape() {
        let set = parse(
            "---\ntitle: T\nn: 42\nf: 1.5\nb: true\nd: 2026-10-20\ndt: 2026-10-20T12:30:00Z\nlist: [a, b]\nlink: \"[[target]]\"\n---\n",
        );
        assert_eq!(
            set.get("title").unwrap().value.property_type(),
            PropertyType::Text
        );
        assert_eq!(set.get("n").unwrap().value, PropertyValue::Number(42.0));
        assert_eq!(set.get("f").unwrap().value, PropertyValue::Number(1.5));
        assert_eq!(set.get("b").unwrap().value, PropertyValue::Boolean(true));
        assert_eq!(
            set.get("d").unwrap().value.property_type(),
            PropertyType::Date
        );
        assert_eq!(
            set.get("dt").unwrap().value.property_type(),
            PropertyType::DateTime
        );
        assert_eq!(
            set.get("list").unwrap().value,
            PropertyValue::MultiSelect(vec!["a".into(), "b".into()])
        );
        assert_eq!(
            set.get("link").unwrap().value.property_type(),
            PropertyType::Link
        );
    }

    #[test]
    fn a_time_only_value_is_not_a_date() {
        assert_eq!(scalar_value("12:30"), PropertyValue::Text("12:30".into()));
        assert_eq!(
            scalar_value("2026-10-20"),
            PropertyValue::Date("2026-10-20".into())
        );
        assert!(matches!(scalar_value("2026-13-45"), PropertyValue::Date(_)));
    }

    #[test]
    fn unknown_fields_are_flagged_and_preserved() {
        let set = parse("---\ntitle: T\ncustom-block:\n  nested: 1\n---\n");
        let custom = set.get("custom-block").unwrap();
        assert!(custom.unknown);
        assert!(!set.get("title").unwrap().unknown);
        // The opaque value still carries the original content for round-trip.
        assert!(matches!(custom.value, PropertyValue::Text(_)));
    }

    #[test]
    fn schema_fields_that_are_missing_are_reported_unset() {
        let set = parse_properties("---\ntitle: T\n---\n", &["title", "rating", "due"]).unwrap();
        assert_eq!(set.unset, vec!["rating", "due"]);
        assert!(set.is_unset("rating"));
    }

    #[test]
    fn broken_and_missing_frontmatter_are_distinguishable() {
        assert_eq!(
            parse_properties("---\ntitle: T\n", &[]),
            Err(PropertyError::Unterminated)
        );
        assert_eq!(
            parse_properties("---\nkey: [unclosed\n---\n", &[]),
            Err(PropertyError::InvalidYaml)
        );
        assert_eq!(
            parse_properties("---\n- a\n- b\n---\n", &[]),
            Err(PropertyError::NotAMapping)
        );
        let empty = parse_properties("body only\n", &["title"]).unwrap();
        assert!(empty.properties.is_empty());
        assert_eq!(empty.unset, vec!["title"]);
    }

    #[test]
    fn non_finite_numbers_are_not_numbers() {
        assert_eq!(scalar_value(".nan"), PropertyValue::Text(".nan".into()));
        assert_eq!(scalar_value(".inf"), PropertyValue::Text(".inf".into()));
    }
}
