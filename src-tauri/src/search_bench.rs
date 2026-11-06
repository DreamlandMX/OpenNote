//! Explicit, release-only host measurement; never run against a user's vault.
use super::*;
use crate::search_job::SearchJobHost;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const NOTES: usize = 10_000;
const SAMPLES: usize = 120;

fn timing(samples: &[f64]) -> serde_json::Value {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[(sorted.len() as f64 * p).ceil() as usize - 1];
    json!({ "samples": sorted.len(), "p50Ms": percentile(0.5), "p95Ms": percentile(0.95), "maxMs": sorted.last().unwrap() })
}

fn digest(root: &Path, paths: &[String]) -> String {
    let mut hash = Sha256::new();
    for path in paths {
        let bytes = fs::read(root.join(path)).unwrap();
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

#[test]
#[ignore = "10k synthetic filesystem benchmark; run explicitly in release with --nocapture"]
fn host_10k_measurement() {
    assert!(
        !cfg!(debug_assertions),
        "measure an optimized release build"
    );
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let sizes = [1024, 4096, 8192, 16384, 102400];
    let mut paths = Vec::with_capacity(NOTES);
    let mut total_bytes = 0;
    for number in 0..NOTES {
        let folder = format!("notes/group-{:02}", number % 20);
        fs::create_dir_all(root.join(&folder)).unwrap();
        let path = format!("{folder}/Note-{number:05}.md");
        let id = uuid::Uuid::from_u128(number as u128 + 1).to_string();
        let mut source = format!("---\nopennote-id: {id}\ntype: study\ntags: [research, team-{}]\n---\n# Knowledge base {number}\n\nUnique marker-{number:05}. İstanbul'da düğün. Tiếng Việt có dấu. 京都旅行。🧪\n\n", number % 8);
        let paragraph = "The knowledge base connects project planning and research. Capture and classify ideas, review milestones and connect related notes.\n\n";
        let remaining = sizes[number % sizes.len()] - source.len();
        source.push_str(&paragraph.repeat(remaining / paragraph.len()));
        source.extend(std::iter::repeat_n(
            'x',
            sizes[number % sizes.len()] - source.len(),
        ));
        total_bytes += source.len();
        fs::write(root.join(&path), source).unwrap();
        paths.push(path);
    }
    let source_hash = digest(root, &paths);
    eprintln!("10k: synthetic files ready, {total_bytes} bytes");
    let started = Instant::now();
    let mut storage = VaultStorage::create(root, "Synthetic host benchmark").unwrap();
    let initialize_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(storage.list().unwrap().len(), NOTES);
    let vault_id = storage.vault().vault_id.clone();
    let host = Arc::new(Mutex::new(SearchHost::default()));
    let jobs = SearchJobHost::default();
    let started = Instant::now();
    let empty = search(
        &host.lock().unwrap(),
        &storage,
        &vault_id,
        &SearchRequest::new(""),
    )
    .unwrap();
    let empty_cold_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(empty.items.is_empty());
    assert_eq!(
        host.lock().unwrap().status().unwrap().state,
        IndexState::Unavailable
    );
    eprintln!("10k: initialized; empty cold query returned without building index");
    let started = Instant::now();
    let cold = search(
        &host.lock().unwrap(),
        &storage,
        &vault_id,
        &SearchRequest::new("knowledge"),
    )
    .unwrap();
    let cold_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(cold.index_status.indexed_notes, NOTES);
    assert_eq!(cold.items.len(), 50);
    eprintln!("10k: cold index ready in {cold_ms:.1} ms");
    assert_eq!(cold.index_status.cache_persisted, Some(true));
    let cache_bytes = std::fs::metadata(root.join(".opennote/index.sqlite"))
        .unwrap()
        .len();
    let started = Instant::now();
    let restored = search(
        &SearchHost::default(),
        &storage,
        &vault_id,
        &SearchRequest::new("knowledge"),
    )
    .unwrap();
    let restart_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(
        restored, cold,
        "a fresh host must reproduce the persisted query and cursor"
    );
    eprintln!("10k: fresh host restored from SQLite in {restart_ms:.1} ms");
    let cursor = cold.next_cursor.clone().unwrap();
    let mut cases = Vec::new();
    for query in [
        "knowledge",
        "project research",
        "京都",
        "İstanbul",
        "Tiếng Việt",
        "marker-09999",
        "absent-token-abc123",
        "",
    ] {
        cases.push(SearchRequest::new(query));
    }
    let mut filtered = SearchRequest::new("knowledge");
    filtered.filters.folders = vec!["notes/group-00".into()];
    filtered.filters.tags = vec!["team-0".into(), "team-4".into()];
    filtered.filters.types = vec!["study".into()];
    cases.push(filtered);
    let mut second_page = SearchRequest::new("knowledge");
    second_page.cursor = Some(cursor);
    cases.push(second_page);
    let request_json: Vec<_> = cases
        .iter()
        .map(|request| serde_json::to_vec(request).unwrap())
        .collect();
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut per_case = vec![Vec::new(); cases.len()];
    let mut background_samples = Vec::with_capacity(SAMPLES);
    let mut background_per_case = vec![Vec::new(); cases.len()];
    for number in 0..SAMPLES {
        let case = number % cases.len();
        let started = Instant::now();
        let request: SearchRequest = serde_json::from_slice(&request_json[case]).unwrap();
        let page = search(&host.lock().unwrap(), &storage, &vault_id, &request).unwrap();
        let response = serde_json::to_vec(&page).unwrap();
        let decoded: SearchPage = serde_json::from_slice(&response).unwrap();
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        samples.push(elapsed);
        per_case[case].push(elapsed);
        assert_eq!(decoded.index_status.indexed_notes, NOTES);
        match case {
            5 => {
                assert_eq!(decoded.items.len(), 1);
                assert_eq!(decoded.items[0].relative_path, paths[9999]);
            }
            6 | 7 => assert!(decoded.items.is_empty()),
            _ => assert_eq!(decoded.items.len(), 50),
        }
        if case == 8 {
            assert!(decoded
                .items
                .iter()
                .all(|note| note.relative_path.starts_with("notes/group-00/")));
        }
        if case == 9 {
            assert_ne!(decoded.items[0].note_id, cold.items[0].note_id);
        }
        let started = Instant::now();
        let worker_request: SearchRequest = serde_json::from_slice(&request_json[case]).unwrap();
        let worker_root = root.to_path_buf();
        let worker_vault = vault_id.clone();
        let summaries = storage.list().unwrap();
        let worker_host = host.clone();
        let initial = jobs
            .start(vault_id.clone(), summaries.len(), move |job| {
                let mut progress =
                    |phase, done, total, unreadable| job.progress(phase, done, total, unreadable);
                let prepared = worker_host.lock().unwrap().prepare_controlled(
                    &worker_root,
                    &worker_vault,
                    &summaries,
                    &worker_request,
                    true,
                    &mut progress,
                )?;
                let result = worker_host.lock().unwrap().finish_controlled(
                    prepared,
                    &worker_request,
                    &mut progress,
                );
                result
            })
            .unwrap();
        let mut observed = initial;
        while observed.status == "running" {
            std::thread::sleep(Duration::from_millis(100));
            observed = jobs.status(&vault_id, &observed.job_id, false).unwrap();
            let payload = serde_json::to_vec(&observed).unwrap();
            let _: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        }
        let background_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(observed.status, "completed");
        assert_eq!(
            observed.page.unwrap(),
            decoded,
            "background job must return exactly the ordinary query page"
        );
        background_samples.push(background_ms);
        background_per_case[case].push(background_ms);
    }
    let note = storage
        .list()
        .unwrap()
        .into_iter()
        .find(|note| note.relative_path == paths[9999])
        .unwrap();
    eprintln!("10k: {SAMPLES} warm search samples collected");
    let mut open_samples = Vec::with_capacity(60);
    for _ in 0..60 {
        let started = Instant::now();
        let document = storage.read(&note.note_id).unwrap();
        let bytes = serde_json::to_vec(&document).unwrap();
        let decoded: onote_domain::NoteDocument = serde_json::from_slice(&bytes).unwrap();
        open_samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(decoded.source.len(), 102400);
        assert!(decoded.source.contains("marker-09999"));
    }
    let started = Instant::now();
    let scan = VaultStorage::scan_catalog(root).unwrap();
    let scan_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let refreshed = storage.refresh_catalog(scan).unwrap();
    let reconcile_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(refreshed.notes.len(), NOTES);
    assert!(refreshed.changes.is_empty());
    assert!(refreshed.findings.is_empty());
    assert_eq!(
        digest(root, &paths),
        source_hash,
        "measurement must not rewrite Markdown"
    );
    let cases: Vec<_> = cases.iter().zip(per_case).zip(background_per_case).enumerate().map(|(i, ((request, samples), background))| json!({ "case": i, "query": request.query, "timing": timing(&samples), "backgroundTiming": timing(&background) })).collect();
    println!(
        "HOST_10K_MEASUREMENT={}",
        json!({
            "schemaVersion": 1, "notes": NOTES, "sourceBytes": total_bytes, "sizesBytes": sizes,
            "scope": "real temporary Windows vault, production M02/M11/M05 and request/response JSON; excludes WebView IPC, debounce, layout and painting",
            "profile": "release", "target": std::env::consts::ARCH, "os": std::env::consts::OS,
        "initializeMs": initialize_ms, "coldEmptyQueryMs": empty_cold_ms, "coldIndexMs": cold_ms, "restartFromSQLiteMs": restart_ms, "cacheFileBytes": cache_bytes, "warmHostSearch": timing(&samples),
            "backgroundJobObserved": timing(&background_samples), "backgroundObservationIntervalMs": 100, "backgroundP95WithinTarget": timing(&background_samples)["p95Ms"].as_f64().unwrap() <= 500.0,
            "cases": cases, "open100KiB": timing(&open_samples), "catalogScanMs": scan_ms, "catalogReconcileMs": reconcile_ms,
            "markdownDigestBeforeAfter": source_hash, "markdownUnchanged": true,
            "p01ThresholdMs": 500, "warmHostP95WithinTarget": timing(&samples)["p95Ms"].as_f64().unwrap() <= 500.0,
            "openHostP95WithinTarget": timing(&open_samples)["p95Ms"].as_f64().unwrap() <= 500.0
        })
    );
}
