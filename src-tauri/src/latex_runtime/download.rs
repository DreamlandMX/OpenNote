//! HTTPS download of the fixed engine archive through the system's WinHTTP
//! (system TLS and proxy settings, no extra crates).
//!
//! Bounded on every side: HTTPS only (and no HTTPS→HTTP redirect), a hard byte
//! limit, an overall deadline, and cancellation between reads. The caller
//! verifies the SHA-256 of what comes back; nothing here trusts the bytes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    NotHttps,
    Cancelled,
    TimedOut,
    TooLarge,
    Status(u32),
    Network(u32),
}

/// `https://host/path` only; the URL comes from the fixed runtime manifest.
fn split(url: &str) -> Result<(String, String), DownloadError> {
    let rest = url
        .strip_prefix("https://")
        .ok_or(DownloadError::NotHttps)?;
    let (host, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    if host.is_empty() || host.contains(['@', ':', ' ']) {
        return Err(DownloadError::NotHttps);
    }
    Ok((host.to_owned(), path.to_owned()))
}

#[cfg(windows)]
pub fn download(
    url: &str,
    max_bytes: u64,
    deadline: Instant,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<Vec<u8>, DownloadError> {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::*;

    struct Internet(*mut c_void);
    impl Drop for Internet {
        fn drop(&mut self) {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let network = || DownloadError::Network(unsafe { GetLastError() });
    let open = |handle: *mut c_void| {
        if handle.is_null() {
            Err(network())
        } else {
            Ok(Internet(handle))
        }
    };

    let (host, path) = split(url)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(DownloadError::Cancelled);
    }
    let agent = wide("OpenNote-LaTeX-Prepare/1");
    let session = open(unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    })?;
    let protocols = WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3;
    let redirects = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;
    unsafe {
        WinHttpSetOption(
            session.0,
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            &protocols as *const u32 as *const c_void,
            4,
        );
        WinHttpSetOption(
            session.0,
            WINHTTP_OPTION_REDIRECT_POLICY,
            &redirects as *const u32 as *const c_void,
            4,
        );
        // Each blocking step returns within 20 s, so cancel is honoured.
        WinHttpSetTimeouts(session.0, 20_000, 20_000, 20_000, 20_000);
    }
    let host_wide = wide(&host);
    let connection = open(unsafe {
        WinHttpConnect(
            session.0,
            host_wide.as_ptr(),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        )
    })?;
    let verb = wide("GET");
    let object = wide(&path);
    let request = open(unsafe {
        WinHttpOpenRequest(
            connection.0,
            verb.as_ptr(),
            object.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    if unsafe { WinHttpSendRequest(request.0, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) } == 0
        || unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) } == 0
    {
        return Err(network());
    }
    let mut status = 0u32;
    let mut length = 4u32;
    if unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut length,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(network());
    }
    if status != 200 {
        return Err(DownloadError::Status(status));
    }

    let mut body = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(DownloadError::TimedOut);
        }
        let mut read = 0u32;
        if unsafe {
            WinHttpReadData(
                request.0,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut read,
            )
        } == 0
        {
            return Err(network());
        }
        if read == 0 {
            return Ok(body);
        }
        if body.len() as u64 + u64::from(read) > max_bytes {
            return Err(DownloadError::TooLarge);
        }
        body.extend_from_slice(&buffer[..read as usize]);
        progress(body.len() as u64);
    }
}

#[cfg(not(windows))]
pub fn download(
    url: &str,
    _max_bytes: u64,
    _deadline: Instant,
    _cancel: &AtomicBool,
    _progress: impl FnMut(u64),
) -> Result<Vec<u8>, DownloadError> {
    split(url)?;
    Err(DownloadError::Network(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_https_urls_are_accepted() {
        assert_eq!(
            split("https://github.com/a/b.zip").unwrap(),
            ("github.com".to_owned(), "/a/b.zip".to_owned())
        );
        for url in [
            "http://github.com/a",
            "file:///C:/a.zip",
            "https://user@github.com/a",
            "https://github.com:8443/a",
            "https:///a",
        ] {
            assert!(split(url).is_err(), "{url}");
        }
    }

    #[test]
    #[ignore = "needs internet access"]
    fn the_engine_archive_downloads_within_its_limits() {
        let manifest = super::super::environment::fixed_manifest();
        let deadline = Instant::now() + std::time::Duration::from_secs(300);
        let bytes = download(
            &manifest.engine.url,
            manifest.engine.archive_bytes,
            deadline,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(bytes.len() as u64, manifest.engine.archive_bytes);
        // One byte less than the archive is a refusal, not a truncated file.
        let short = download(
            &manifest.engine.url,
            manifest.engine.archive_bytes - 1,
            deadline,
            &AtomicBool::new(false),
            |_| {},
        );
        assert_eq!(short, Err(DownloadError::TooLarge));
    }
}
