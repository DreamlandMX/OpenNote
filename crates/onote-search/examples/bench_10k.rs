//! F12-A01 / P-01 measurement harness: warm search over a 10k-note synthetic
//! vault. It is an example rather than a test because a debug-profile timing
//! assertion is noise; run it in release and record the printed numbers.
//!
//! `cargo run --release -p onote-search --example bench_10k`

use std::time::Instant;

use onote_search::{SearchIndex, SearchRequest, SearchSource};

const NOTES: usize = 10_000;
const WARM_QUERIES: usize = 120;

fn synthetic_note(index: usize) -> SearchSource {
    let folder = match index % 8 {
        0 => "notes/work",
        1 => "notes/home",
        2 => "notes/study",
        3 => "notes/travel",
        4 => "archive/2024",
        5 => "archive/2025",
        6 => "inbox",
        _ => "projects",
    };
    let title = match index % 5 {
        0 => format!("Knowledge base {index}"),
        1 => format!("Learning method {index}"),
        2 => format!("Project plan {index}"),
        3 => format!("Meeting notes {index}"),
        _ => format!("京都旅行 {index}"),
    };
    let body = format!(
        "# {title}\n\n\
         This note describes the knowledge base workflow for item {index}. \
         A good knowledge base is not a pile of documents but a growing system. \
         Capture, classify and connect: {index}.\n\n\
         - milestone {index}\n\
         - owner team-{index}\n\n\
         ## Details\n\n\
         The search index should stay warm and return the first page quickly. \
         Turkish: İstanbul'da düğün. Vietnamese: Tiếng Việt có dấu. 日本語の文章。\n\n\
         ```rust\nfn item_{index}() -> usize {{ {index} }}\n```\n"
    );
    SearchSource::new(
        format!("note-{index:05}"),
        format!("{folder}/note-{index:05}.md"),
        title,
        body,
    )
}

fn percentile(values: &mut [u128], percentile: f64) -> u128 {
    values.sort_unstable();
    let rank = ((values.len() as f64 - 1.0) * percentile).round() as usize;
    values[rank]
}

fn main() {
    let notes: Vec<SearchSource> = (0..NOTES).map(synthetic_note).collect();
    let build_started = Instant::now();
    let index = SearchIndex::rebuild(&notes, 1, 0);
    let build_ms = build_started.elapsed().as_millis();
    println!(
        "cold rebuild: {NOTES} notes in {build_ms} ms ({} indexed)",
        index.status().indexed_notes
    );

    // Warm the code paths once before timing.
    let _ = index
        .query(&SearchRequest::new("knowledge base"))
        .expect("warmup query");

    let queries = [
        "knowledge",
        "knowledge base",
        "learning method",
        "project plan",
        "京都",
        "İstanbul",
        "Tiếng Việt",
        "milestone",
        "team-42",
        "item 9000",
        "growing system",
        "first page",
    ];
    let mut timings = Vec::with_capacity(WARM_QUERIES);
    for position in 0..WARM_QUERIES {
        let query = queries[position % queries.len()];
        let started = Instant::now();
        let page = index
            .query(&SearchRequest::new(query))
            .expect("benchmark query");
        let elapsed = started.elapsed().as_micros();
        assert!(!page.items.is_empty(), "query {query} found nothing");
        timings.push(elapsed);
    }

    let p50 = percentile(&mut timings.clone(), 0.50);
    let p95 = percentile(&mut timings, 0.95);
    let max = timings.iter().copied().max().unwrap_or(0);
    println!(
        "warm query: {WARM_QUERIES} queries, p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        p50 as f64 / 1000.0,
        p95 as f64 / 1000.0,
        max as f64 / 1000.0
    );
    println!(
        "P-01 target (warm 10k P95 <= 500 ms): {}",
        if p95 <= 500_000 { "PASS" } else { "FAIL" }
    );
}
