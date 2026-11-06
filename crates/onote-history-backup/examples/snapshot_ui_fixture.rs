use onote_vault_storage::VaultStorage;
use std::fs;
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn main() {
    let fixture = std::env::temp_dir().join(format!("opennote-backup-ui-{}", op()));
    fs::create_dir(&fixture).unwrap();
    let source = fixture.join("source");
    let destination = fixture.join("backups");
    let restore_parent = fixture.join("restore-here");
    for directory in [&source, &destination, &restore_parent] {
        fs::create_dir(directory).unwrap();
    }
    let mut vault = VaultStorage::create(&source, "备份界面验收").unwrap();
    let note = vault
        .create_note(
            "中文笔记.md",
            "---\ncustom: 保留\n---\n# 备份预览\n\n只使用合成数据。\n\n[共享附件](shared.bin)",
            &op(),
        )
        .unwrap();
    vault
        .create_note("second.md", "# Second note\n\nBackup fixture.", &op())
        .unwrap();
    fs::write(source.join("shared.bin"), [0, 255, 1, 13, 10]).unwrap();
    assert_eq!(vault.list().unwrap().len(), 2);
    assert_eq!(
        fs::read(source.join("中文笔记.md")).unwrap(),
        note.source.as_bytes()
    );
    println!(
        "{}",
        serde_json::json!({"fixture":fixture,"source":source,"destination":destination,"restoreParent":restore_parent,"vaultId":vault.vault().vault_id,"noteId":note.note_id})
    );
}
