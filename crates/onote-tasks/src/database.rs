//! M06 database projection (F22/F23).
//!
//! A "database" is a view over ordinary notes: the properties live in each
//! note's frontmatter, and this module only decides which notes are rows, which
//! columns exist, and how they are filtered, sorted and grouped.
//!
//! Two rules shape it:
//!
//! * a note without the property shows as unset, never as 0 or an empty string
//!   (F22-R01), so "no value" and "value of zero" stay distinguishable;
//! * sorting and filtering are total and stable: equal values are broken by the
//!   note id, so the same vault always produces the same order (F23 §4).
//!
//! Nothing here reads or writes a file, and no view definition is persisted yet.

use onote_metadata_links::PropertyValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One row: a note and its projected cell values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub note_id: String,
    pub title: String,
    pub relative_path: String,
    pub source_revision: u64,
    pub source_hash: String,
    pub read_only: bool,
    /// Cell values in the order of `Table::columns`.
    pub cells: Vec<CellValue>,
}

/// A cell is either a real value or explicitly unset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum CellValue {
    /// No such property on this note (F22-R01).
    Unset,
    Text(String),
    Number(f64),
    Boolean(bool),
    /// Calendar day `YYYY-MM-DD`; never converted through a time zone.
    Date(String),
    DateTime(String),
    Select(String),
    MultiSelect(Vec<String>),
    Link(String),
}

impl CellValue {
    fn from_property(value: &PropertyValue) -> Self {
        match value {
            PropertyValue::Text(text) => Self::Text(text.clone()),
            PropertyValue::Number(number) => Self::Number(*number),
            PropertyValue::Boolean(flag) => Self::Boolean(*flag),
            PropertyValue::Date(date) => Self::Date(date.clone()),
            PropertyValue::DateTime(instant) => Self::DateTime(instant.clone()),
            PropertyValue::Select(choice) => Self::Select(choice.clone()),
            PropertyValue::MultiSelect(choices) => Self::MultiSelect(choices.clone()),
            PropertyValue::Link(target) => Self::Link(target.clone()),
            PropertyValue::Unset => Self::Unset,
        }
    }

    /// Whether this cell has no value. Sorted separately so the direction never
    /// flips it: unset stays last ascending *and* descending (F23 §4).
    fn is_unset(&self) -> bool {
        matches!(self, Self::Unset)
    }

    /// The comparable key for a set value; unset uses an empty key.
    fn sort_key(&self) -> (u8, String) {
        match self {
            Self::Unset => (1, String::new()),
            Self::Number(number) => (0, format!("{number:020.6}")),
            Self::Boolean(flag) => (0, if *flag { "1" } else { "0" }.to_owned()),
            Self::Date(date) | Self::DateTime(date) => (0, date.clone()),
            Self::Text(text) | Self::Select(text) | Self::Link(text) => (0, text.to_lowercase()),
            Self::MultiSelect(choices) => (0, choices.join(",").to_lowercase()),
        }
    }

    /// A comparable string for equality filters, lower-cased for text.
    pub fn match_text(&self) -> Option<String> {
        match self {
            Self::Unset => None,
            Self::Number(number) => Some(format!("{number}")),
            Self::Boolean(flag) => Some(if *flag { "true" } else { "false" }.to_owned()),
            Self::Date(value) | Self::DateTime(value) => Some(value.clone()),
            Self::Text(text) | Self::Select(text) | Self::Link(text) => Some(text.to_lowercase()),
            Self::MultiSelect(choices) => Some(choices.join(",").to_lowercase()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub key: String,
    /// Property type as parsed from the notes, or `text` when only unset values
    /// were seen (a column with no values still has to be displayed).
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterOp {
    Equals,
    NotEquals,
    Contains,
    IsSet,
    IsUnset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filter {
    pub key: String,
    pub op: FilterOp,
    /// Ignored for `IsSet` / `IsUnset`.
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sort {
    /// A column key, or the reserved `title` / `path`.
    pub key: String,
    pub direction: SortDirection,
}

/// A note as the caller read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowSource {
    pub note_id: String,
    pub title: String,
    pub relative_path: String,
    pub source_revision: u64,
    pub source_hash: String,
    pub read_only: bool,
    pub properties: BTreeMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// Rows removed by the filters; the UI shows this so "hidden" is not
    /// mistaken for "deleted" (F23-R04).
    pub hidden: usize,
}

fn kind_name(value: &PropertyValue) -> &'static str {
    match value {
        PropertyValue::Text(_) => "text",
        PropertyValue::Number(_) => "number",
        PropertyValue::Boolean(_) => "boolean",
        PropertyValue::Date(_) => "date",
        PropertyValue::DateTime(_) => "datetime",
        PropertyValue::Select(_) => "select",
        PropertyValue::MultiSelect(_) => "multiselect",
        PropertyValue::Link(_) => "link",
        PropertyValue::Unset => "unset",
    }
}

/// The ordered set of column keys, from the requested order then the data.
///
/// An explicit `requested` order wins so the user's column order is stable; any
/// property not mentioned is appended in alphabetical order, because dropping it
/// would hide data that exists in the notes.
pub fn columns_for(sources: &[RowSource], requested: &[String]) -> Vec<Column> {
    let mut kinds: BTreeMap<String, String> = BTreeMap::new();
    for source in sources {
        for (key, value) in &source.properties {
            if matches!(value, PropertyValue::Unset) {
                continue;
            }
            kinds
                .entry(key.clone())
                .or_insert_with(|| kind_name(value).to_owned());
        }
    }
    let mut columns: Vec<Column> = Vec::new();
    for key in requested {
        columns.push(Column {
            key: key.clone(),
            kind: kinds.get(key).cloned().unwrap_or_else(|| "text".to_owned()),
        });
    }
    for (key, kind) in kinds {
        if !requested.iter().any(|existing| existing == &key) {
            columns.push(Column { key, kind });
        }
    }
    columns
}

fn matches(cell: &CellValue, filter: &Filter) -> bool {
    match filter.op {
        FilterOp::IsUnset => matches!(cell, CellValue::Unset),
        FilterOp::IsSet => !matches!(cell, CellValue::Unset),
        FilterOp::Equals => cell
            .match_text()
            .is_some_and(|text| text == filter.value.to_lowercase()),
        FilterOp::NotEquals => cell
            .match_text()
            .is_none_or(|text| text != filter.value.to_lowercase()),
        // A multi-select cell matches when any of its choices contains the text,
        // so filtering by one label does not require an exact list match.
        FilterOp::Contains => match cell {
            CellValue::MultiSelect(choices) => choices
                .iter()
                .any(|choice| choice.to_lowercase().contains(&filter.value.to_lowercase())),
            other => other
                .match_text()
                .is_some_and(|text| text.contains(&filter.value.to_lowercase())),
        },
    }
}

/// Builds a table from the given rows.
///
/// Filters combine with AND across different keys and OR within one key, which
/// is the rule F23 §4 states.
pub fn build_table(
    sources: &[RowSource],
    requested_columns: &[String],
    filters: &[Filter],
    sort: Option<&Sort>,
) -> Table {
    let columns = columns_for(sources, requested_columns);
    let mut rows: Vec<Row> = Vec::new();
    let mut hidden = 0usize;
    for source in sources {
        let mut keep = true;
        for column in &columns {
            let key_filters: Vec<&Filter> = filters
                .iter()
                .filter(|filter| filter.key == column.key)
                .collect();
            if key_filters.is_empty() {
                continue;
            }
            let cell = source
                .properties
                .get(&column.key)
                .map(CellValue::from_property)
                .unwrap_or(CellValue::Unset);
            // OR within one key: any filter passing keeps the row.
            if !key_filters.iter().any(|filter| matches(&cell, filter)) {
                keep = false;
                break;
            }
        }
        if !keep {
            hidden += 1;
            continue;
        }
        rows.push(Row {
            note_id: source.note_id.clone(),
            title: source.title.clone(),
            relative_path: source.relative_path.clone(),
            source_revision: source.source_revision,
            source_hash: source.source_hash.clone(),
            read_only: source.read_only,
            cells: columns
                .iter()
                .map(|column| {
                    source
                        .properties
                        .get(&column.key)
                        .map(CellValue::from_property)
                        .unwrap_or(CellValue::Unset)
                })
                .collect(),
        });
    }

    if let Some(sort) = sort {
        let index = columns.iter().position(|column| column.key == sort.key);
        rows.sort_by(|left, right| {
            let ordering = match (index, sort.key.as_str()) {
                (Some(index), _) => {
                    let left_cell = left.cells.get(index).unwrap_or(&CellValue::Unset);
                    let right_cell = right.cells.get(index).unwrap_or(&CellValue::Unset);
                    match (left_cell.is_unset(), right_cell.is_unset()) {
                        (true, true) => std::cmp::Ordering::Equal,
                        // Unset always sorts last, whichever direction is asked
                        // for; reversing it would put empty rows first.
                        (true, false) => std::cmp::Ordering::Greater,
                        (false, true) => std::cmp::Ordering::Less,
                        (false, false) => {
                            let ordering = left_cell.sort_key().cmp(&right_cell.sort_key());
                            match sort.direction {
                                SortDirection::Ascending => ordering,
                                SortDirection::Descending => ordering.reverse(),
                            }
                        }
                    }
                }
                (None, key) => {
                    let ordering = match key {
                        "title" => left.title.to_lowercase().cmp(&right.title.to_lowercase()),
                        "path" => left
                            .relative_path
                            .to_lowercase()
                            .cmp(&right.relative_path.to_lowercase()),
                        // An unknown sort key must not silently reorder anything.
                        _ => std::cmp::Ordering::Equal,
                    };
                    match sort.direction {
                        SortDirection::Ascending => ordering,
                        SortDirection::Descending => ordering.reverse(),
                    }
                }
            };
            // A stable tiebreak keeps the order reproducible (F23 §4).
            ordering.then_with(|| left.note_id.cmp(&right.note_id))
        });
    } else {
        rows.sort_by(|left, right| left.note_id.cmp(&right.note_id));
    }

    Table {
        columns,
        rows,
        hidden,
    }
}

/// One board column: a select value and the rows in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// The select value, or None for the "no value" column (F23-R01).
    pub value: Option<String>,
    pub note_ids: Vec<String>,
}

/// Groups rows by a select property, keeping an explicit "no value" group.
///
/// The ungrouped bucket comes last so it cannot be mistaken for a real choice,
/// and a row whose value is not a select or multi-select is ungrouped rather
/// than guessed (F23-R01).
pub fn group_by_select(table: &Table, key: &str) -> Vec<Group> {
    let Some(index) = table.columns.iter().position(|column| column.key == key) else {
        return vec![Group {
            value: None,
            note_ids: table.rows.iter().map(|row| row.note_id.clone()).collect(),
        }];
    };
    let mut groups: Vec<Group> = Vec::new();
    let mut ungrouped: Vec<String> = Vec::new();
    for row in &table.rows {
        match row.cells.get(index) {
            Some(CellValue::Select(value)) => match groups
                .iter_mut()
                .find(|group| group.value.as_deref() == Some(value.as_str()))
            {
                Some(group) => group.note_ids.push(row.note_id.clone()),
                None => groups.push(Group {
                    value: Some(value.clone()),
                    note_ids: vec![row.note_id.clone()],
                }),
            },
            Some(CellValue::MultiSelect(choices)) if choices.len() == 1 => {
                let value = choices[0].clone();
                match groups
                    .iter_mut()
                    .find(|group| group.value.as_deref() == Some(value.as_str()))
                {
                    Some(group) => group.note_ids.push(row.note_id.clone()),
                    None => groups.push(Group {
                        value: Some(value),
                        note_ids: vec![row.note_id.clone()],
                    }),
                }
            }
            // Unset, a non-select value, or a multi-value cell: no single column
            // would be truthful, so it stays ungrouped.
            _ => ungrouped.push(row.note_id.clone()),
        }
    }
    groups.sort_by(|left, right| left.value.cmp(&right.value));
    if !ungrouped.is_empty() {
        groups.push(Group {
            value: None,
            note_ids: ungrouped,
        });
    }
    groups
}

/// Rows that carry a date in `key`, for the calendar view (F23-R02).
///
/// Only the named date property is used: a calendar that silently fell back to
/// another date field would move a note to a day the author never wrote.
pub fn calendar_cells(table: &Table, key: &str) -> Vec<(String, String)> {
    let Some(index) = table.columns.iter().position(|column| column.key == key) else {
        return Vec::new();
    };
    let mut cells: Vec<(String, String)> = table
        .rows
        .iter()
        .filter_map(|row| match row.cells.get(index) {
            Some(CellValue::Date(date)) => Some((date.clone(), row.note_id.clone())),
            _ => None,
        })
        .collect();
    cells.sort_by(|left, right| {
        (left.0.as_str(), left.1.as_str()).cmp(&(right.0.as_str(), right.1.as_str()))
    });
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, title: &str, entries: &[(&str, PropertyValue)]) -> RowSource {
        RowSource {
            note_id: id.to_owned(),
            title: title.to_owned(),
            relative_path: format!("{title}.md"),
            source_revision: 1,
            source_hash: "hash-1".into(),
            read_only: false,
            properties: entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        }
    }

    #[test]
    fn projected_rows_keep_the_exact_source_version_and_readonly_state() {
        let mut original = source("a", "A", &[]);
        original.source_revision = 7;
        original.source_hash = "exact-source-hash".into();
        original.read_only = true;
        let table = build_table(&[original], &[], &[], None);
        let wire = serde_json::to_value(&table.rows[0]).unwrap();
        assert_eq!(wire["sourceRevision"], 7);
        assert_eq!(wire["sourceHash"], "exact-source-hash");
        assert_eq!(wire["readOnly"], true);
    }

    fn sample() -> Vec<RowSource> {
        vec![
            source(
                "n1",
                "Alpha",
                &[
                    ("status", PropertyValue::Select("doing".into())),
                    ("score", PropertyValue::Number(10.0)),
                    ("due", PropertyValue::Date("2026-10-20".into())),
                ],
            ),
            source(
                "n2",
                "Beta",
                &[
                    ("status", PropertyValue::Select("todo".into())),
                    ("score", PropertyValue::Number(0.0)),
                ],
            ),
            source(
                "n3",
                "Gamma",
                &[("status", PropertyValue::Select("doing".into()))],
            ),
        ]
    }

    #[test]
    fn a_missing_property_is_unset_not_zero() {
        // F22-R01: Beta has score 0 and Gamma has no score; they must differ.
        let table = build_table(&sample(), &["score".into()], &[], None);
        let beta = table.rows.iter().find(|row| row.note_id == "n2").unwrap();
        let gamma = table.rows.iter().find(|row| row.note_id == "n3").unwrap();
        assert_eq!(beta.cells[0], CellValue::Number(0.0));
        assert_eq!(gamma.cells[0], CellValue::Unset);
    }

    #[test]
    fn same_titled_notes_keep_distinct_rows_and_ids() {
        // F22-A01: rows are keyed by note id, so two notes named "同名" in
        // different folders appear separately instead of collapsing into one.
        let sources = vec![
            RowSource {
                note_id: "a".into(),
                title: "同名".into(),
                relative_path: "甲/同名.md".into(),
                source_revision: 1,
                source_hash: "hash-1".into(),
                read_only: false,
                properties: BTreeMap::new(),
            },
            RowSource {
                note_id: "b".into(),
                title: "同名".into(),
                relative_path: "乙/同名.md".into(),
                source_revision: 1,
                source_hash: "hash-1".into(),
                read_only: false,
                properties: BTreeMap::new(),
            },
        ];
        let table = build_table(&sources, &[], &[], None);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].note_id, "a");
        assert_eq!(table.rows[1].note_id, "b");
        assert_eq!(table.rows[0].title, table.rows[1].title);
        assert_ne!(table.rows[0].relative_path, table.rows[1].relative_path);
    }

    #[test]
    fn requested_columns_come_first_and_extra_columns_are_appended() {
        let table = build_table(&sample(), &["due".into(), "status".into()], &[], None);
        let keys: Vec<&str> = table
            .columns
            .iter()
            .map(|column| column.key.as_str())
            .collect();
        assert_eq!(keys, vec!["due", "status", "score"]);
        // A column with no values still reports a type rather than vanishing.
        assert_eq!(table.columns[0].kind, "date");
    }

    #[test]
    fn filters_combine_and_across_keys_and_or_within_one_key() {
        let filters = vec![
            Filter {
                key: "status".into(),
                op: FilterOp::Equals,
                value: "doing".into(),
            },
            Filter {
                key: "status".into(),
                op: FilterOp::Equals,
                value: "todo".into(),
            },
        ];
        // OR within one key: both statuses survive.
        let table = build_table(&sample(), &[], &filters, None);
        assert_eq!(table.rows.len(), 3);

        // AND across keys: only Alpha is "doing" and dated.
        let filters = vec![
            Filter {
                key: "status".into(),
                op: FilterOp::Equals,
                value: "doing".into(),
            },
            Filter {
                key: "due".into(),
                op: FilterOp::IsSet,
                value: String::new(),
            },
        ];
        let table = build_table(&sample(), &[], &filters, None);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].note_id, "n1");
        assert_eq!(table.hidden, 2);
    }

    #[test]
    fn filtering_only_hides_rows() {
        // F23-R04: hidden counts rows the filters removed; nothing is deleted.
        let filters = vec![Filter {
            key: "status".into(),
            op: FilterOp::Equals,
            value: "todo".into(),
        }];
        let table = build_table(&sample(), &[], &filters, None);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.hidden, 2);
        assert_eq!(table.rows.len() + table.hidden, 3);
    }

    #[test]
    fn unset_values_sort_last_in_both_directions() {
        let ascending = build_table(
            &sample(),
            &["score".into()],
            &[],
            Some(&Sort {
                key: "score".into(),
                direction: SortDirection::Ascending,
            }),
        );
        assert_eq!(
            ascending
                .rows
                .iter()
                .map(|row| row.note_id.as_str())
                .collect::<Vec<_>>(),
            vec!["n2", "n1", "n3"]
        );
        let descending = build_table(
            &sample(),
            &["score".into()],
            &[],
            Some(&Sort {
                key: "score".into(),
                direction: SortDirection::Descending,
            }),
        );
        assert_eq!(
            descending
                .rows
                .iter()
                .map(|row| row.note_id.as_str())
                .collect::<Vec<_>>(),
            vec!["n1", "n2", "n3"],
            "an unset value stays last when descending"
        );
    }

    #[test]
    fn equal_values_are_broken_by_note_id_for_a_stable_order() {
        let equal = vec![
            source(
                "b",
                "B",
                &[("status", PropertyValue::Select("same".into()))],
            ),
            source(
                "a",
                "A",
                &[("status", PropertyValue::Select("same".into()))],
            ),
        ];
        let table = build_table(
            &equal,
            &[],
            &[],
            Some(&Sort {
                key: "status".into(),
                direction: SortDirection::Ascending,
            }),
        );
        assert_eq!(
            table
                .rows
                .iter()
                .map(|row| row.note_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn an_unknown_sort_key_keeps_the_id_order() {
        let table = build_table(
            &sample(),
            &[],
            &[],
            Some(&Sort {
                key: "nope".into(),
                direction: SortDirection::Ascending,
            }),
        );
        assert_eq!(
            table
                .rows
                .iter()
                .map(|row| row.note_id.as_str())
                .collect::<Vec<_>>(),
            vec!["n1", "n2", "n3"]
        );
    }

    #[test]
    fn sorting_by_title_is_available_without_a_column() {
        let table = build_table(
            &sample(),
            &[],
            &[],
            Some(&Sort {
                key: "title".into(),
                direction: SortDirection::Descending,
            }),
        );
        assert_eq!(
            table
                .rows
                .iter()
                .map(|row| row.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Gamma", "Beta", "Alpha"]
        );
    }

    #[test]
    fn contains_matches_any_choice_of_a_multiselect() {
        let sources = vec![
            source(
                "n1",
                "A",
                &[(
                    "tags",
                    PropertyValue::MultiSelect(vec!["work".into(), "home".into()]),
                )],
            ),
            source(
                "n2",
                "B",
                &[("tags", PropertyValue::MultiSelect(vec!["home".into()]))],
            ),
        ];
        let table = build_table(
            &sources,
            &[],
            &[Filter {
                key: "tags".into(),
                op: FilterOp::Contains,
                value: "wor".into(),
            }],
            None,
        );
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].note_id, "n1");
    }

    #[test]
    fn not_equals_keeps_unset_rows() {
        // A row with no value is not equal to the excluded value, so filtering it
        // out would hide data the user did not exclude.
        let table = build_table(
            &sample(),
            &[],
            &[Filter {
                key: "due".into(),
                op: FilterOp::NotEquals,
                value: "2026-10-20".into(),
            }],
            None,
        );
        assert_eq!(table.rows.len(), 2);
    }

    #[test]
    fn boards_keep_an_ungrouped_column_last() {
        // F23-R01: a row without a value goes to "no value", not a guessed group.
        let sources = vec![
            source(
                "n1",
                "A",
                &[("status", PropertyValue::Select("todo".into()))],
            ),
            source("n2", "B", &[]),
            source(
                "n3",
                "C",
                &[("status", PropertyValue::Select("doing".into()))],
            ),
        ];
        let table = build_table(&sources, &["status".into()], &[], None);
        let groups = group_by_select(&table, "status");
        assert_eq!(groups.last().unwrap().value, None);
        assert_eq!(groups.last().unwrap().note_ids, vec!["n2"]);
        assert_eq!(groups.len(), 3);
    }

    #[test]
    fn a_non_select_value_is_ungrouped_rather_than_guessed() {
        let sources = vec![source("n1", "A", &[("status", PropertyValue::Number(3.0))])];
        let table = build_table(&sources, &["status".into()], &[], None);
        let groups = group_by_select(&table, "status");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].value, None);
    }

    #[test]
    fn a_multiselect_with_one_choice_joins_that_group() {
        let sources = vec![source(
            "n1",
            "A",
            &[("status", PropertyValue::MultiSelect(vec!["todo".into()]))],
        )];
        let table = build_table(&sources, &["status".into()], &[], None);
        assert_eq!(
            group_by_select(&table, "status")[0].value.as_deref(),
            Some("todo")
        );
    }

    #[test]
    fn calendar_uses_only_the_named_date_field() {
        // F23-R02: never fall back to another date field.
        let sources = vec![
            source(
                "n1",
                "A",
                &[("due", PropertyValue::Date("2026-10-20".into()))],
            ),
            source(
                "n2",
                "B",
                &[("other", PropertyValue::Date("2026-10-21".into()))],
            ),
        ];
        let table = build_table(&sources, &["due".into(), "other".into()], &[], None);
        assert_eq!(
            calendar_cells(&table, "due"),
            vec![("2026-10-20".to_owned(), "n1".to_owned())]
        );
        assert!(calendar_cells(&table, "missing").is_empty());
    }

    #[test]
    fn an_empty_vault_produces_no_rows() {
        let table = build_table(&[], &["status".into()], &[], None);
        assert!(table.rows.is_empty());
        assert_eq!(table.columns.len(), 1);
        assert_eq!(table.hidden, 0);
    }
}
