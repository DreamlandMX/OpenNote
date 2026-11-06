use onote_history_backup::BackupStore;
use onote_vault_storage::VaultStorage;
use sha2::{Digest, Sha256};
use std::{fs, sync::atomic::AtomicBool, time::Instant};
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn main() {
    let fixture = std::env::temp_dir().join(format!("opennote-snapshot-scale-{}", op()));
    fs::create_dir(&fixture).unwrap();
    let source = fixture.join("source");
    fs::create_dir(&source).unwrap();
    let destination = fixture.join("backups");
    fs::create_dir(&destination).unwrap();
    let target = fixture.join("restored");
    let vault = VaultStorage::create(&source, "万篇合成库").unwrap();
    drop(vault);
    let started = Instant::now();
    for index in 0..10_000 {
        let text = format!(
            "---\nopennote-id: {}\ncustom: 数据{index}\n---\n# 笔记 {index}\n\n合成正文。\n",
            op()
        );
        fs::write(source.join(format!("笔记-{index:05}.md")), text).unwrap();
    }
    fs::write(source.join("shared.bin"), [0, 255, 1, 13, 10]).unwrap();
    let vault = VaultStorage::open(&source).unwrap();
    let seed_ms = started.elapsed().as_millis();
    let backup = BackupStore::open(&source, &vault.vault().vault_id, &destination).unwrap();
    let started = Instant::now();
    let snapshot = backup
        .create(&vault, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let create_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let plan = backup
        .plan_restore(&snapshot.snapshot_id, &target, &AtomicBool::new(false))
        .unwrap();
    let preview_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let result = backup
        .apply_restore(&plan, &op(), &AtomicBool::new(false), |_| {})
        .unwrap();
    let restore_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let restored = VaultStorage::open(&target).unwrap();
    let notes = restored.list().unwrap();
    assert_eq!(notes.len(), 10_000);
    assert_eq!(result.note_count, 10_000);
    let mut dataset = Sha256::new();
    for note in &notes {
        let actual = fs::read(target.join(&note.relative_path)).unwrap();
        assert_eq!(actual, fs::read(source.join(&note.relative_path)).unwrap());
        assert_eq!(hex::encode(Sha256::digest(&actual)), note.content_hash);
        assert_eq!(note.local_revision, 1);
        dataset.update(&actual);
    }
    for note in notes.iter().step_by(997) {
        let document = restored.read(&note.note_id).unwrap();
        assert_eq!(document.content_hash, note.content_hash);
    }
    assert_eq!(
        fs::read(target.join("shared.bin")).unwrap(),
        [0, 255, 1, 13, 10]
    );
    assert_eq!(backup.verify(&snapshot.snapshot_id).unwrap(), snapshot);
    println!(
        "{}",
        serde_json::json!({"fixture":fixture,"noteCount":notes.len(),"fileByteAndHashChecks":notes.len(),"nativeStorageReadSamples":notes.iter().step_by(997).count(),"datasetSha256":hex::encode(dataset.finalize()),"seedMs":seed_ms,"createMs":create_ms,"previewMs":preview_ms,"restoreMs":restore_ms,"independentVerificationMs":started.elapsed().as_millis(),"result":result})
    );
}
