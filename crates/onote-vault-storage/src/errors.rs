use onote_domain::{ErrorCode, ServiceError};

pub(crate) fn err(code: ErrorCode, key: &str, params: &[(&str, String)]) -> ServiceError {
    ServiceError {
        code,
        message_key: key.to_owned(),
        params: params
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
        retryable: false,
        operation_id: None,
        retry_after_seconds: None,
    }
}

pub(crate) fn io(e: &std::io::Error, key: &str) -> ServiceError {
    #[cfg(windows)]
    if matches!(e.raw_os_error(), Some(32 | 33)) {
        let mut failure = err(ErrorCode::FileLocked, key, &[]);
        failure.retryable = true;
        return failure;
    }
    let code = match e.kind() {
        std::io::ErrorKind::StorageFull => ErrorCode::DiskFull,
        std::io::ErrorKind::NotFound => ErrorCode::ObjectNotFound,
        _ => ErrorCode::IoDenied,
    };
    err(code, key, &[])
}

pub(crate) fn database(e: rusqlite::Error) -> ServiceError {
    match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DiskFull) => {
            err(ErrorCode::DiskFull, "error.stateWriteFailed", &[])
        }
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            file_locked()
        }
        _ => err(ErrorCode::IoDenied, "error.stateWriteFailed", &[]),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sqlite_full_and_busy_preserve_actionable_error_codes() {
        for (native, expected) in [
            (
                rusqlite::ffi::SQLITE_FULL,
                onote_domain::ErrorCode::DiskFull,
            ),
            (
                rusqlite::ffi::SQLITE_BUSY,
                onote_domain::ErrorCode::FileLocked,
            ),
        ] {
            let failure = super::database(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(native),
                None,
            ));
            assert_eq!(failure.code, expected);
            if expected == onote_domain::ErrorCode::FileLocked {
                assert!(failure.retryable);
            }
        }
    }
}

/// FILE_LOCKED with retryable=true: another writer currently holds the
/// vault gate's OS lock.
pub(crate) fn file_locked() -> ServiceError {
    let mut e = err(ErrorCode::FileLocked, "error.vaultWriterLocked", &[]);
    e.retryable = true;
    e
}

pub(crate) type SeaRes<T> = Result<T, ServiceError>;
