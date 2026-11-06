use onote_history_backup::BackupStore;
use onote_vault_storage::VaultStorage;
use std::{fs, sync::atomic::AtomicBool};
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn main() {
    let fixture = std::env::temp_dir().join(format!("opennote-cleanup-ui-{}", op()));
    fs::create_dir(&fixture).unwrap();
    let source = fixture.join("source");
    let destination = fixture.join("backups");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    let mut storage = VaultStorage::create(&source, "中断清理合成验收").unwrap();
    let note = storage
        .create_note(
            "合成笔记.md",
            "# Synthetic only\n\n[asset](shared.bin)",
            &op(),
        )
        .unwrap();
    fs::write(source.join("shared.bin"), [0, 255, 1, 13, 10]).unwrap();
    let backup = BackupStore::open(&source, &storage.vault().vault_id, &destination).unwrap();
    let cancel = AtomicBool::new(false);
    for _ in 0..3 {
        backup.create(&storage, &op(), &cancel, |_| {}).unwrap();
    }
    let plan = backup.plan_retention(1, &cancel, |_| {}).unwrap();
    let operation = op();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = backup.apply_cleanup(&plan, &operation, &cancel, |progress| {
            if progress.phase == "cleaning" {
                panic!("synthetic interruption after the first removed file");
            }
        });
    }))
    .is_err());
    assert_eq!(backup.pending_cleanups().unwrap().len(), 1);
    assert_eq!(
        backup.verify(&plan.retained[0].snapshot_id).unwrap(),
        plan.retained[0]
    );
    assert_eq!(
        fs::read(source.join("合成笔记.md")).unwrap(),
        note.source.as_bytes()
    );
    println!(
        "{}",
        serde_json::json!({"fixture":fixture,"source":source,"destination":destination,"operationId":operation,"retainedSnapshot":plan.retained[0],"originalCandidates":plan.candidates,"vaultId":storage.vault().vault_id})
    );
}
