//! F52: the real SyncTeX format, for jumping between source lines and PDF pages.
//!
//! A record is `<type><file>,<line>:<x>,<y>[:<width>,<height>,<depth>]`, with
//! coordinates in scaled points from the page's top left. Only what a jump needs is
//! kept. A file this parser does not understand yields no records, and then no jump:
//! the reader stays usable and nothing points at a wrong line (F52 §10).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Keeps a hostile or runaway file from filling memory.
const MAX_RECORDS: usize = 4_000_000;
/// Scaled points per PDF point: 65536 per TeX point, 72.27 TeX points per inch.
const SP_PER_BP: f64 = 65536.0 * 72.27 / 72.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A horizontal box: the only record with a useful width.
    Box,
    /// A point inside a line: glue, kern, math or a character box.
    Point,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Record {
    page: u32,
    file: u32,
    line: u32,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    depth: i64,
    kind: Kind,
}

/// A spot on a PDF page, in PDF points from the page's top left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfSpot {
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A place in the source, by the path as the engine wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSpot {
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SyncIndex {
    inputs: BTreeMap<u32, String>,
    records: Vec<Record>,
    unit: f64,
    x_offset: i64,
    y_offset: i64,
}

/// `<file>,<line>:<x>,<y>[:<width>,<height>,<depth>]`.
fn parse_fields(rest: &str) -> Option<(u32, u32, i64, i64, i64, i64, i64)> {
    let mut groups = rest.split(':');
    let (file, line) = groups.next()?.split_once(',')?;
    let (x, y) = groups.next()?.split_once(',')?;
    let (mut width, mut height, mut depth) = (0, 0, 0);
    if let Some(size) = groups.next() {
        // A kern gives only a width; a box gives all three.
        let mut parts = size.split(',');
        width = parts.next()?.parse().ok()?;
        if let Some(value) = parts.next() {
            height = value.parse().ok()?;
            depth = parts.next()?.parse().ok()?;
        }
    }
    Some((
        file.parse().ok()?,
        line.parse().ok()?,
        x.parse().ok()?,
        y.parse().ok()?,
        width,
        height,
        depth,
    ))
}

/// Reads a decompressed `.synctex` file.
pub fn parse_synctex(text: &str) -> SyncIndex {
    let mut index = SyncIndex {
        unit: 1.0,
        ..SyncIndex::default()
    };
    let mut magnification = 1000.0f64;
    let mut declared_unit = 1.0f64;
    let mut page = 0u32;
    let mut in_content = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Input:") {
            if let Some((tag, path)) = rest.split_once(':') {
                if let (Ok(tag), false) = (tag.parse::<u32>(), path.is_empty()) {
                    index.inputs.insert(tag, path.to_owned());
                }
            }
            continue;
        }
        if !in_content {
            if let Some(value) = line.strip_prefix("Magnification:") {
                magnification = value.parse().unwrap_or(1000.0);
            } else if let Some(value) = line.strip_prefix("Unit:") {
                declared_unit = value.parse().unwrap_or(1.0);
            } else if let Some(value) = line.strip_prefix("X Offset:") {
                index.x_offset = value.parse().unwrap_or(0);
            } else if let Some(value) = line.strip_prefix("Y Offset:") {
                index.y_offset = value.parse().unwrap_or(0);
            } else if line == "Content:" {
                in_content = true;
            }
            continue;
        }
        if line.starts_with("Postamble:") {
            break;
        }
        let mut characters = line.chars();
        let Some(first) = characters.next() else {
            continue;
        };
        let rest = characters.as_str();
        match first {
            '{' => page = rest.parse().unwrap_or(0),
            '}' => page = 0,
            '(' | 'h' | 'k' | 'g' | '$' | 'x' if page > 0 => {
                if index.records.len() == MAX_RECORDS {
                    continue;
                }
                let Some((file, source_line, x, y, width, height, depth)) = parse_fields(rest)
                else {
                    continue;
                };
                index.records.push(Record {
                    page,
                    file,
                    line: source_line,
                    x,
                    y,
                    width,
                    height,
                    depth,
                    kind: if first == '(' { Kind::Box } else { Kind::Point },
                });
            }
            _ => {}
        }
    }
    index.unit = declared_unit * magnification / 1000.0;
    index
}

impl SyncIndex {
    /// Every source path the file names, for mapping onto the project.
    pub fn inputs(&self) -> impl Iterator<Item = (u32, &str)> {
        self.inputs.iter().map(|(tag, path)| (*tag, path.as_str()))
    }

    fn to_bp(&self, value: i64, offset: i64) -> f64 {
        ((value + offset) as f64) * self.unit / SP_PER_BP
    }

    /// Where a source line landed. `file` is the path as written in the SyncTeX file.
    /// The nearest line with output stands in for a line that produced none, such as
    /// a comment, but no further than `reach` lines away.
    pub fn forward(&self, file: &str, line: u32, reach: u32) -> Option<PdfSpot> {
        let tags: Vec<u32> = self
            .inputs
            .iter()
            .filter(|(_, path)| path.as_str() == file)
            .map(|(tag, _)| *tag)
            .collect();
        let candidates = self
            .records
            .iter()
            .filter(|record| tags.contains(&record.file) && record.line > 0);
        let nearest = candidates
            .clone()
            .map(|record| record.line.abs_diff(line))
            .min()?;
        if nearest > reach {
            return None;
        }
        let on_line: Vec<&Record> = candidates
            .filter(|record| record.line.abs_diff(line) == nearest)
            .collect();
        // A line can span pages; the first appearance is where a reader starts.
        let page = on_line.iter().map(|record| record.page).min()?;
        let chosen = on_line
            .iter()
            .filter(|record| record.page == page)
            .min_by_key(|record| (record.y, record.x))?;
        // A point has no size. The box it sits in is the visible line, whichever
        // file that box happens to be tagged with.
        let shape = self.enclosing_box(chosen).unwrap_or(chosen);
        let height = shape.height.max(0);
        Some(PdfSpot {
            page,
            x: self.to_bp(shape.x, self.x_offset),
            y: self.to_bp(shape.y - height, self.y_offset),
            width: self.to_bp(shape.width.max(0), 0),
            height: self.to_bp(height + shape.depth.max(0), 0),
        })
    }

    /// The tightest box around a record, on its page.
    fn enclosing_box(&self, inner: &Record) -> Option<&Record> {
        self.records
            .iter()
            .filter(|record| {
                record.page == inner.page
                    && record.kind == Kind::Box
                    && record.width > 0
                    && record.height + record.depth > 0
                    && inner.x >= record.x
                    && inner.x <= record.x + record.width
                    && inner.y >= record.y - record.height
                    && inner.y <= record.y + record.depth
            })
            .min_by_key(|record| record.width.saturating_mul(record.height + record.depth))
    }

    /// The source behind a point on a page, in PDF points. The smallest box holding
    /// the point wins; otherwise the nearest thing on the page, since a click in a
    /// margin still means that line. Files `keep` rejects, such as packages, do not
    /// count, so a click never lands in a file the user does not have.
    pub fn inverse(
        &self,
        page: u32,
        x: f64,
        y: f64,
        keep: impl Fn(&str) -> bool,
    ) -> Option<SourceSpot> {
        let kept: Vec<u32> = self
            .inputs
            .iter()
            .filter(|(_, path)| keep(path))
            .map(|(tag, _)| *tag)
            .collect();
        let on_page = || {
            self.records
                .iter()
                .filter(|record| record.page == page && record.line > 0)
        };
        let is_kept = |record: &&Record| kept.contains(&record.file);

        // The box under the point tells which printed line it is. Its own tag is not
        // the answer: TeX names the file that was open when the paragraph ended, so
        // a line from an \\input file can sit in a box tagged with the main file.
        let holding = on_page()
            .filter(|record| record.kind == Kind::Box)
            .filter(|record| {
                let (left, right) = (
                    self.to_bp(record.x, self.x_offset),
                    self.to_bp(record.x + record.width.max(0), self.x_offset),
                );
                let (top, bottom) = (
                    self.to_bp(record.y - record.height.max(0), self.y_offset),
                    self.to_bp(record.y + record.depth.max(0), self.y_offset),
                );
                x >= left && x <= right && y >= top && y <= bottom
            })
            .min_by_key(|record| record.width.saturating_mul(record.height + record.depth));
        if let Some(line_box) = holding {
            // What sits on that line records where it really came from.
            let closest = on_page()
                .filter(is_kept)
                .filter(|record| record.kind == Kind::Point && record.y == line_box.y)
                .min_by(|left, right| {
                    let distance =
                        |record: &Record| (self.to_bp(record.x, self.x_offset) - x).abs();
                    distance(left).total_cmp(&distance(right))
                });
            if let Some(record) =
                closest.or(Some(line_box).filter(|record| kept.contains(&record.file)))
            {
                return Some(SourceSpot {
                    file: self.inputs.get(&record.file)?.clone(),
                    line: record.line,
                });
            }
        }

        // A margin or a gap still means the nearest line, with lines counting for
        // more than columns.
        let nearest = on_page().filter(is_kept).min_by(|left, right| {
            let distance = |record: &Record| {
                let top = self.to_bp(record.y - record.height.max(0), self.y_offset);
                let bottom = self.to_bp(record.y + record.depth.max(0), self.y_offset);
                let start = self.to_bp(record.x, self.x_offset);
                let end = start + self.to_bp(record.width.max(0), 0);
                let dy = if y < top {
                    top - y
                } else if y > bottom {
                    y - bottom
                } else {
                    0.0
                };
                let dx = if x < start {
                    start - x
                } else if x > end {
                    x - end
                } else {
                    0.0
                };
                dy * 4.0 + dx
            };
            distance(left).total_cmp(&distance(right))
        })?;
        Some(SourceSpot {
            file: self.inputs.get(&nearest.file)?.clone(),
            line: nearest.line,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real Tectonic file: two lines on page 1, one on page 2.
    const SAMPLE: &str = "SyncTeX Version:1\n\
Input:1:C:\\job\\input\\main.tex\n\
Input:2:\n\
Input:3:C:\\cache\\article.cls\n\
Input:4:C:\\job\\input\\sections\\a.tex\n\
Output:pdf\n\
Magnification:1000\n\
Unit:1\n\
X Offset:0\n\
Y Offset:0\n\
Content:\n\
!1368\n\
{1\n\
[1,25:4736287,52685372:29760291,47949085,0\n\
(3,9:4736287,2171087:0,0,0\n\
k3,9:4736287,2171087:140368\n\
)\n\
(1,14:4736287,6553600:29760291,655360,131072\n\
h1,14:4736287,6553600:0,0,0\n\
g1,14:5000000,6553600\n\
)\n\
(1,15:4736287,7864320:29760291,655360,131072\n\
h1,15:4736287,7864320:0,0,0\n\
)\n\
]\n\
!3026\n\
}1\n\
{2\n\
(4,3:4736287,6553600:29760291,655360,131072\n\
h4,3:4736287,6553600:0,0,0\n\
)\n\
}2\n\
Input:5:C:\\job\\input\\late.tex\n\
Postamble:\n\
Count:109\n";

    const MAIN: &str = "C:\\job\\input\\main.tex";
    const SECTION: &str = "C:\\job\\input\\sections\\a.tex";

    fn in_project(path: &str) -> bool {
        path.contains("\\input\\")
    }

    #[test]
    fn a_source_line_finds_its_box_in_pdf_points() {
        let index = parse_synctex(SAMPLE);
        let spot = index.forward(MAIN, 14, 3).unwrap();
        assert_eq!(spot.page, 1);
        // 4736287 sp is one inch from the page's left edge.
        assert!((spot.x - 72.0).abs() < 0.01, "{}", spot.x);
        assert!(spot.width > 400.0, "{}", spot.width);
        // The box top is its baseline minus its height.
        let expected = (6553600.0 - 655360.0) / SP_PER_BP;
        assert!((spot.y - expected).abs() < 0.01, "{}", spot.y);
        assert!(spot.height > 9.0 && spot.height < 12.0, "{}", spot.height);
    }

    #[test]
    fn each_file_and_page_is_kept_apart() {
        let index = parse_synctex(SAMPLE);
        assert_eq!(index.forward(SECTION, 3, 0).unwrap().page, 2);
        assert!(index.forward("C:\\other.tex", 14, 3).is_none());
    }

    #[test]
    fn a_line_with_no_output_uses_its_neighbour_within_reach_only() {
        let index = parse_synctex(SAMPLE);
        assert_eq!(index.forward(MAIN, 13, 3).unwrap().page, 1);
        assert!(index.forward(MAIN, 200, 3).is_none());
    }

    #[test]
    fn a_click_finds_the_line_under_it() {
        let index = parse_synctex(SAMPLE);
        let y = 6553600.0 / SP_PER_BP - 2.0;
        assert_eq!(
            index.inverse(1, 100.0, y, in_project),
            Some(SourceSpot {
                file: MAIN.to_owned(),
                line: 14
            })
        );
        let lower = 7864320.0 / SP_PER_BP - 2.0;
        assert_eq!(index.inverse(1, 100.0, lower, in_project).unwrap().line, 15);
        assert_eq!(
            index.inverse(2, 100.0, y, in_project),
            Some(SourceSpot {
                file: SECTION.to_owned(),
                line: 3
            })
        );
    }

    #[test]
    fn a_click_in_the_margin_means_the_nearest_line() {
        let index = parse_synctex(SAMPLE);
        let y = 7864320.0 / SP_PER_BP + 40.0;
        assert_eq!(index.inverse(1, 5.0, y, in_project).unwrap().line, 15);
    }

    #[test]
    fn a_click_never_lands_in_a_package_file() {
        let index = parse_synctex(SAMPLE);
        let spot = index
            .inverse(1, 72.0, 2171087.0 / SP_PER_BP, in_project)
            .unwrap();
        assert_eq!(spot.file, MAIN);
        assert!(index.inverse(9, 72.0, 72.0, in_project).is_none());
        assert!(index.inverse(1, 72.0, 72.0, |_| false).is_none());
    }

    /// As Tectonic writes it: the line's box names the main file, its contents the
    /// `\\input` file they came from.
    const NESTED: &str = "Input:1:main.tex\nInput:2:one.tex\nContent:\n{1\n\
(1,9:4736287,6553600:29760291,655360,131072\n\
h2,4:5000000,6553600:0,0,0\n\
g2,4:6000000,6553600\n\
k2,5:9000000,6553600:100\n\
)\n}1\n";

    #[test]
    fn a_line_from_an_input_file_answers_with_that_file() {
        let index = parse_synctex(NESTED);
        let y = 6553600.0 / SP_PER_BP - 2.0;
        let near_start = index
            .inverse(1, 6000000.0 / SP_PER_BP, y, |_| true)
            .unwrap();
        assert_eq!(
            near_start,
            SourceSpot {
                file: "one.tex".to_owned(),
                line: 4
            }
        );
        let near_end = index
            .inverse(1, 9000000.0 / SP_PER_BP + 1.0, y, |_| true)
            .unwrap();
        assert_eq!(near_end.line, 5, "the nearest thing on the line wins");
    }

    #[test]
    fn a_line_from_an_input_file_is_marked_as_its_whole_printed_line() {
        let spot = parse_synctex(NESTED).forward("one.tex", 4, 0).unwrap();
        assert!(spot.height > 9.0, "the box gives it a height: {spot:?}");
        assert!(spot.width > 400.0, "{spot:?}");
    }

    #[test]
    fn inputs_named_after_the_content_still_count() {
        let index = parse_synctex(SAMPLE);
        assert!(index
            .inputs()
            .any(|(tag, path)| tag == 5 && path.ends_with("late.tex")));
        assert!(
            !index.inputs().any(|(tag, _)| tag == 2),
            "an empty name is not an input"
        );
    }

    #[test]
    fn offsets_and_magnification_are_honoured() {
        let text = "Input:1:a.tex\nMagnification:2000\nUnit:1\nX Offset:65781\nY Offset:0\nContent:\n{1\n(1,1:0,131072:65781,65536,0\n)\n}1\n";
        let spot = parse_synctex(text).forward("a.tex", 1, 0).unwrap();
        assert!((spot.x - 2.0).abs() < 0.01, "{}", spot.x);
        assert!((spot.width - 2.0).abs() < 0.01, "{}", spot.width);
    }

    #[test]
    fn junk_yields_no_jump_rather_than_a_wrong_one() {
        for text in [
            "",
            "not a synctex file\n",
            "Content:\n{1\n(x,y:z\n",
            "Input:1:a.tex\nContent:\n{1\n(1,1\n}1\n",
        ] {
            let index = parse_synctex(text);
            assert!(index.forward("a.tex", 1, 100).is_none());
            assert!(index.inverse(1, 0.0, 0.0, |_| true).is_none());
        }
    }
}
