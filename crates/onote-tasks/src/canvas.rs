//! M06 canvas document model (F21).
//!
//! A canvas is an open, exportable JSON document that arranges *references* to
//! notes and free text. Three rules shape this module:
//!
//! * a reference card keeps the note id and never copies the body, so editing the
//!   canvas can never fork a note (F21-R01);
//! * geometry is validated before it is accepted — a non-finite or sub-minimum
//!   node is refused rather than stored as a broken layout (F21-R06,
//!   INVALID_GEOMETRY);
//! * unknown fields survive a round trip, so a newer file opened by this build
//!   does not silently lose data (F21 §4).
//!
//! Parsing and serialising live here; persisting the file does not. Storing a
//! `.canvas` file needs M02 to accept a non-Markdown path, which is a separate
//! decision, so this module is deliberately free of any storage call.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Minimum node size in logical pixels from F21 §4.
pub const MIN_NODE_WIDTH: f64 = 120.0;
pub const MIN_NODE_HEIGHT: f64 = 80.0;

/// The canvas format this build writes.
pub const FORMAT_VERSION: &str = "1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GeometryError {
    /// A coordinate or size was NaN or infinite.
    NonFinite,
    /// A node is smaller than the documented minimum.
    TooSmall { width: String, height: String },
    /// An edge names a node that is not in the document.
    DanglingEdge { edge_id: String, missing: String },
    /// Two nodes share an id, so an edge would be ambiguous.
    DuplicateNodeId(String),
    /// The document is not valid JSON.
    Malformed(String),
}

/// One node of a canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasNode {
    pub id: String,
    /// `note`, `text`, `group` or `file` — kept as a string so an unknown type
    /// from a newer file is preserved rather than rejected.
    #[serde(rename = "type")]
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// The referenced note id, for `note` nodes (F21-R01).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Fields this build does not model; carried through unchanged.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasDocument {
    /// `formatVersion` as written; a newer value is preserved, not downgraded.
    #[serde(default = "default_format_version")]
    pub format_version: String,
    #[serde(default)]
    pub nodes: Vec<CanvasNode>,
    #[serde(default)]
    pub edges: Vec<CanvasEdge>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn default_format_version() -> String {
    FORMAT_VERSION.to_owned()
}

impl Default for CanvasDocument {
    fn default() -> Self {
        Self {
            format_version: default_format_version(),
            nodes: Vec::new(),
            edges: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// Parses a `.canvas` document.
pub fn parse(source: &str) -> Result<CanvasDocument, GeometryError> {
    let document: CanvasDocument = serde_json::from_str(source)
        .map_err(|error| GeometryError::Malformed(error.to_string()))?;
    validate(&document)?;
    Ok(document)
}

/// Serialises a document, preserving every field it carried.
pub fn serialize(document: &CanvasDocument) -> Result<String, GeometryError> {
    validate(document)?;
    serde_json::to_string_pretty(document)
        .map_err(|error| GeometryError::Malformed(error.to_string()))
}

/// Refuses a document that cannot be drawn truthfully.
///
/// This runs on read *and* before write: a file edited by another tool could
/// contain a broken layout, and accepting it would make the canvas show
/// something the JSON does not actually say (INVALID_GEOMETRY).
pub fn validate(document: &CanvasDocument) -> Result<(), GeometryError> {
    let mut seen: Vec<&str> = Vec::new();
    for node in &document.nodes {
        if seen.contains(&node.id.as_str()) {
            return Err(GeometryError::DuplicateNodeId(node.id.clone()));
        }
        seen.push(&node.id);
        if !node.x.is_finite()
            || !node.y.is_finite()
            || !node.width.is_finite()
            || !node.height.is_finite()
        {
            return Err(GeometryError::NonFinite);
        }
        // Groups are allowed to be any size; a content node is not.
        if node.kind != "group" && (node.width < MIN_NODE_WIDTH || node.height < MIN_NODE_HEIGHT) {
            return Err(GeometryError::TooSmall {
                width: format!("{}", node.width),
                height: format!("{}", node.height),
            });
        }
    }
    for edge in &document.edges {
        for endpoint in [&edge.from, &edge.to] {
            if !seen.contains(&endpoint.as_str()) {
                return Err(GeometryError::DanglingEdge {
                    edge_id: edge.id.clone(),
                    missing: endpoint.clone(),
                });
            }
        }
    }
    Ok(())
}

/// A note id a canvas refers to, with the nodes that reference it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasReference {
    pub note_id: String,
    pub node_ids: Vec<String>,
}

/// Every distinct note the canvas references, sorted by id.
pub fn references(document: &CanvasDocument) -> Vec<CanvasReference> {
    let mut found: Vec<CanvasReference> = Vec::new();
    for node in &document.nodes {
        let Some(note_id) = node.note_id.as_deref() else {
            continue;
        };
        match found.iter_mut().find(|entry| entry.note_id == note_id) {
            Some(entry) => entry.node_ids.push(node.id.clone()),
            None => found.push(CanvasReference {
                note_id: note_id.to_owned(),
                node_ids: vec![node.id.clone()],
            }),
        }
    }
    found.sort_by(|left, right| left.note_id.cmp(&right.note_id));
    found
}

/// Removes a node and the edges touching it (F21-R02).
///
/// The referenced note is untouched: this function only edits the document it is
/// given, which is what "deleting a card deletes only the view node" means.
pub fn remove_node(document: &mut CanvasDocument, node_id: &str) -> bool {
    let before = document.nodes.len();
    document.nodes.retain(|node| node.id != node_id);
    if document.nodes.len() == before {
        return false;
    }
    document
        .edges
        .retain(|edge| edge.from != node_id && edge.to != node_id);
    true
}

/// A node whose referenced note could not be found (BROKEN_NODE).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokenNode {
    pub node_id: String,
    pub note_id: Option<String>,
}

/// Nodes whose reference does not resolve against the given note ids.
///
/// The node is kept and reported rather than dropped: losing the card would also
/// lose the information about which note it pointed at (BROKEN_NODE).
pub fn broken_nodes(document: &CanvasDocument, known: &[String]) -> Vec<BrokenNode> {
    let mut broken = Vec::new();
    for node in &document.nodes {
        if let Some(note_id) = node.note_id.as_deref() {
            if !known.iter().any(|id| id == note_id) {
                broken.push(BrokenNode {
                    node_id: node.id.clone(),
                    note_id: Some(note_id.to_owned()),
                });
            }
        }
    }
    broken
}

/// What this build's export can and cannot claim (F21-R04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDisclosure {
    /// Always "JSON Canvas" — the format name actually written.
    pub format: String,
    pub format_version: String,
    /// Extensions this build wrote that a plain JSON Canvas reader may ignore.
    pub extensions: Vec<String>,
    /// True because this build writes only the documented core fields.
    pub core_only: bool,
}

/// The disclosure an export must carry.
///
/// It names the format and version and states whether anything outside the core
/// schema was written, so the export never implies compatibility it does not
/// have (F21-R04).
pub fn export_disclosure(document: &CanvasDocument) -> ExportDisclosure {
    let mut extensions: Vec<String> = Vec::new();
    for node in &document.nodes {
        for key in node.extra.keys() {
            if !extensions.contains(key) {
                extensions.push(key.clone());
            }
        }
    }
    for edge in &document.edges {
        for key in edge.extra.keys() {
            if !extensions.contains(key) {
                extensions.push(key.clone());
            }
        }
    }
    for key in document.extra.keys() {
        if !extensions.contains(key) {
            extensions.push(key.clone());
        }
    }
    extensions.sort();
    ExportDisclosure {
        format: "JSON Canvas".to_owned(),
        format_version: document.format_version.clone(),
        core_only: extensions.is_empty(),
        extensions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "formatVersion": "1.0",
      "nodes": [
        {"id":"n1","type":"note","x":0,"y":0,"width":240,"height":160,"noteId":"note-a"},
        {"id":"n2","type":"text","x":300,"y":0,"width":200,"height":120,"text":"hello"},
        {"id":"g1","type":"group","x":-20,"y":-20,"width":60,"height":40}
      ],
      "edges": [{"id":"e1","from":"n1","to":"n2"}]
    }"#;

    #[test]
    fn shared_frontend_wire_fixture_keeps_type_and_flattened_extensions() {
        let source = include_str!("../tests/fixtures/canvas-wire.json");
        let document = parse(source).expect("frontend wire fixture");
        assert_eq!(document.nodes[0].kind, "note");
        assert_eq!(document.nodes[0].extra["kind"], "future-extension");
        let serialized = serialize(&document).expect("serialize IPC document");
        let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(value["nodes"][0]["type"], "note");
        assert_eq!(value["nodes"][0]["kind"], "future-extension");
        assert_eq!(value["future"], serde_json::json!({"nested": [1, 2]}));
        assert_eq!(value["nodes"][0]["futureNode"], serde_json::json!([3]));
        assert_eq!(
            value["edges"][0]["futureEdge"],
            serde_json::json!({"a": true})
        );
        assert!(value.get("extra").is_none());
        assert_eq!(parse(&serialized).unwrap(), document);
    }

    #[test]
    fn parses_a_document_and_keeps_references() {
        let document = parse(SAMPLE).expect("parse");
        assert_eq!(document.format_version, "1.0");
        assert_eq!(document.nodes.len(), 3);
        assert_eq!(
            references(&document),
            vec![CanvasReference {
                note_id: "note-a".to_owned(),
                node_ids: vec!["n1".to_owned()]
            }]
        );
    }

    #[test]
    fn a_reference_card_keeps_the_note_id_and_never_the_body() {
        // F21-R01: the node stores an id; the body stays in the note.
        let document = parse(SAMPLE).expect("parse");
        let note = document.nodes.iter().find(|node| node.id == "n1").unwrap();
        assert_eq!(note.note_id.as_deref(), Some("note-a"));
        assert!(note.text.is_none());
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        // F21 §4: a newer file must not lose data this build does not model.
        let source = r#"{
          "formatVersion": "1.1",
          "futureField": {"nested": [1, 2]},
          "nodes": [{"id":"n1","type":"note","x":0,"y":0,"width":240,"height":160,"noteId":"a","futureNodeField":7}],
          "edges": []
        }"#;
        let document = parse(source).expect("parse");
        let again = parse(&serialize(&document).expect("serialize")).expect("reparse");
        assert_eq!(again.format_version, "1.1");
        assert_eq!(
            again.extra.get("futureField"),
            Some(&serde_json::json!({"nested": [1, 2]}))
        );
        assert_eq!(
            again.nodes[0].extra.get("futureNodeField"),
            Some(&serde_json::json!(7))
        );
    }

    #[test]
    fn non_finite_geometry_is_refused() {
        // JSON cannot express NaN, so an infinite value arrives as a string in a
        // hand-edited file; the parser rejects it as malformed and a programmatic
        // NaN is caught by validate.
        let mut document = CanvasDocument::default();
        document.nodes.push(CanvasNode {
            id: "n1".to_owned(),
            kind: "text".to_owned(),
            x: f64::INFINITY,
            y: 0.0,
            width: 200.0,
            height: 120.0,
            note_id: None,
            text: Some("x".to_owned()),
            label: None,
            color: None,
            extra: BTreeMap::new(),
        });
        assert_eq!(validate(&document), Err(GeometryError::NonFinite));
        assert_eq!(serialize(&document), Err(GeometryError::NonFinite));
    }

    #[test]
    fn a_node_smaller_than_the_minimum_is_refused_but_a_group_may_be() {
        let mut document = CanvasDocument::default();
        document.nodes.push(CanvasNode {
            id: "n1".to_owned(),
            kind: "text".to_owned(),
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 60.0,
            note_id: None,
            text: Some("x".to_owned()),
            label: None,
            color: None,
            extra: BTreeMap::new(),
        });
        assert!(matches!(
            validate(&document),
            Err(GeometryError::TooSmall { .. })
        ));
        document.nodes[0].kind = "group".to_owned();
        assert_eq!(validate(&document), Ok(()));
    }

    #[test]
    fn an_edge_to_an_unknown_node_is_refused() {
        let source = r#"{
          "formatVersion": "1.0",
          "nodes": [{"id":"n1","type":"text","x":0,"y":0,"width":200,"height":120}],
          "edges": [{"id":"e1","from":"n1","to":"ghost"}]
        }"#;
        assert_eq!(
            parse(source),
            Err(GeometryError::DanglingEdge {
                edge_id: "e1".to_owned(),
                missing: "ghost".to_owned()
            })
        );
    }

    #[test]
    fn a_self_loop_is_allowed() {
        // F21 §4: cyclic connections are allowed; only recursion is excluded.
        let source = r#"{
          "formatVersion": "1.0",
          "nodes": [{"id":"n1","type":"text","x":0,"y":0,"width":200,"height":120}],
          "edges": [{"id":"e1","from":"n1","to":"n1"}]
        }"#;
        assert!(parse(source).is_ok());
    }

    #[test]
    fn duplicate_node_ids_are_refused() {
        let source = r#"{
          "formatVersion": "1.0",
          "nodes": [
            {"id":"n1","type":"text","x":0,"y":0,"width":200,"height":120},
            {"id":"n1","type":"text","x":0,"y":0,"width":200,"height":120}
          ],
          "edges": []
        }"#;
        assert_eq!(
            parse(source),
            Err(GeometryError::DuplicateNodeId("n1".to_owned()))
        );
    }

    #[test]
    fn removing_a_card_removes_its_edges_and_leaves_the_note_alone() {
        // F21-R02: the document loses a view node; the note id is only data here.
        let mut document = parse(SAMPLE).expect("parse");
        assert!(remove_node(&mut document, "n1"));
        assert_eq!(document.nodes.len(), 2);
        assert!(document.edges.is_empty());
        // Removing an absent node reports that nothing changed.
        assert!(!remove_node(&mut document, "n1"));
    }

    #[test]
    fn broken_nodes_are_reported_not_dropped() {
        // BROKEN_NODE: keep the card and say which note is missing.
        let document = parse(SAMPLE).expect("parse");
        let broken = broken_nodes(&document, &[]);
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].note_id.as_deref(), Some("note-a"));
        assert!(broken_nodes(&document, &["note-a".to_owned()]).is_empty());
    }

    #[test]
    fn the_export_discloses_its_format_and_any_extensions() {
        // F21-R04: never imply compatibility the export does not have.
        let clean = export_disclosure(&CanvasDocument::default());
        assert_eq!(clean.format, "JSON Canvas");
        assert!(clean.core_only);
        assert!(clean.extensions.is_empty());

        let document = parse(SAMPLE).expect("parse");
        let with_extra = export_disclosure(&CanvasDocument {
            extra: [("vendor".to_owned(), serde_json::json!(true))]
                .into_iter()
                .collect(),
            ..document
        });
        assert!(!with_extra.core_only);
        assert_eq!(with_extra.extensions, vec!["vendor".to_owned()]);
    }

    #[test]
    fn malformed_json_is_refused_without_panicking() {
        assert!(matches!(parse("{"), Err(GeometryError::Malformed(_))));
        assert!(matches!(parse("null"), Err(GeometryError::Malformed(_))));
    }

    #[test]
    fn an_empty_document_is_valid() {
        let document = parse(r#"{"formatVersion":"1.0","nodes":[],"edges":[]}"#).expect("parse");
        assert!(document.nodes.is_empty());
        assert_eq!(validate(&document), Ok(()));
    }
}
