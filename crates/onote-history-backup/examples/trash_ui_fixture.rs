use onote_history_backup::HistoryStore;
use onote_vault_storage::{Failpoint, VaultStorage};
use std::{fs, sync::Arc};
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn main() {
    let root = std::env::temp_dir().join(format!("opennote-trash-ui-{}", op()));
    fs::create_dir(&root).unwrap();
    let mut vault = VaultStorage::create(&root, "回收站界面验收").unwrap();
    let history = Arc::new(HistoryStore::open(&root, &vault.vault().vault_id).unwrap());
    vault.set_revision_archive(history.clone());
    vault.set_trash_archive(history);
    let readable = vault
        .create_note(
            "恢复预览.md",
            "# 需要恢复的笔记\n\n保留中文正文与 **Markdown**。\n\n[共享附件](shared.pdf)",
            &op(),
        )
        .unwrap();
    let damaged = vault
        .create_note("损坏副本.md", "# 损坏案例\n\n合成测试数据。", &op())
        .unwrap();
    let pending = vault
        .create_note("中断操作.md", "# 中断的删除\n\n原始正文不能丢失。", &op())
        .unwrap();
    vault
        .create_note(
            "删除预检.md",
            "# 删除前比较\n\n仅用于预检，不在界面执行删除。",
            &op(),
        )
        .unwrap();
    fs::write(root.join("shared.pdf"), b"synthetic shared attachment").unwrap();
    let first = vault
        .delete_note(&readable.note_id, 0, &readable.content_hash, &op())
        .unwrap();
    let broken = vault
        .delete_note(&damaged.note_id, 0, &damaged.content_hash, &op())
        .unwrap();
    fs::write(
        root.join(".opennote/trash")
            .join(format!("{}.json", broken.object_id)),
        b"{}",
    )
    .unwrap();
    let operation = op();
    vault.push_failpoint(Failpoint::AfterPrepared);
    assert!(vault
        .delete_note(&pending.note_id, 0, &pending.content_hash, &operation)
        .is_err());
    assert_eq!(vault.list_trash().unwrap().len(), 2);
    assert_eq!(vault.list_trash_operations().unwrap().len(), 1);
    assert!(root.join("中断操作.md").is_file());
    assert_eq!(
        fs::read(root.join("shared.pdf")).unwrap(),
        b"synthetic shared attachment"
    );
    println!(
        "{}",
        serde_json::json!({ "vault": root, "readableTrashId": first.object_id, "pendingOperationId": operation })
    );
}
