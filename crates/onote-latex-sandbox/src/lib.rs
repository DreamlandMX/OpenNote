//! Restricted execution for the local LaTeX engine (F45/F53, ARCH-08).
//!
//! The engine is untrusted input-processing code: it runs project sources that
//! may try `\write18`, read arbitrary files or reach the network. This crate is
//! the only way OpenNote starts it, and it never offers an unrestricted mode:
//!
//! * the process runs in an AppContainer with **no capabilities**, so it has no
//!   network (loopback included) and can only open what is explicitly granted;
//! * only the listed directories are granted, read-only or read-write; nothing
//!   else (the vault, the user profile) is;
//! * it is created suspended, put into a Job Object that forbids breakaway and
//!   kills everything on close, and only then resumed, so no child escapes;
//! * cancel and timeout terminate the whole job, and the run reports how many
//!   processes were still alive afterwards (it must be zero);
//! * stdout and stderr are read concurrently and bounded, so a chatty engine
//!   can neither block on a full pipe nor exhaust memory.
//!
//! If any of these cannot be set up the run fails; there is no fallback.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[cfg(windows)]
mod windows;

/// The app's own AppContainer profile for the LaTeX engine. Tests use their
/// own throwaway profiles; only this one is used by the product.
pub const APP_PROFILE: &str = "app.opennote.desktop.latex";

/// A separate profile for preparing resources, the only runs that may reach
/// the internet. It never shares a profile (and so never shares granted
/// directories) with compiles, which stay on `APP_PROFILE` without network.
pub const PREPARE_PROFILE: &str = "app.opennote.desktop.latex.prepare";

/// Every profile the product creates, for removal.
pub const PRODUCT_PROFILES: [&str; 2] = [APP_PROFILE, PREPARE_PROFILE];

/// Network access of the restricted process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// No capability at all: no internet, no LAN, no loopback.
    None,
    /// The `internetClient` capability only: outbound internet connections.
    /// Without `privateNetworkClientServer` the LAN stays unreachable, and an
    /// AppContainer never reaches loopback.
    InternetClientOnly,
}

/// What to run and what it may touch.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    /// AppContainer profile name; app-specific and reused across runs.
    pub profile: String,
    /// Absolute path of the program; its directory must be in `read_dirs`.
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Working directory; must be in `write_dirs`.
    pub cwd: PathBuf,
    /// Granted read and execute.
    pub read_dirs: Vec<PathBuf>,
    /// Granted read, write and delete.
    pub write_dirs: Vec<PathBuf>,
    /// Granted `FILE_READ_ATTRIBUTES` on the directory object itself only:
    /// no listing, no file reads, nothing inherited by its contents. This is
    /// what lets the engine confirm a folder exists (see `profile_folders`).
    pub attribute_only_dirs: Vec<PathBuf>,
    pub timeout: Duration,
    /// Bytes kept from stdout and from stderr each; the rest is drained.
    pub max_output_bytes: usize,
    /// Extra variables (for example the engine's cache directory). They are
    /// added to the minimal environment and replace a default of the same name.
    pub env: Vec<(String, OsString)>,
    pub network: Network,
    /// The run is stopped once this directory holds more than this many bytes
    /// (checked while it runs), bounding what a download may write.
    pub byte_budget: Option<(PathBuf, u64)>,
}

/// How the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Exited(u32),
    TimedOut,
    Cancelled,
    /// `byte_budget` was exceeded; the run was stopped.
    ByteBudgetExceeded,
}

#[derive(Debug, Clone)]
pub struct SandboxRun {
    pub exit: Exit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// True when either stream exceeded `max_output_bytes`.
    pub truncated: bool,
    /// Processes of the job still alive after it was terminated. Anything but
    /// zero means isolation failed and the caller must not trust the run.
    pub remaining_processes: u32,
}

/// Why a sandboxed run could not be started or supervised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxError {
    /// The setup step that failed, for diagnostics.
    pub stage: &'static str,
    /// The OS error code, when there is one.
    pub code: u32,
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "sandbox {} failed (code {})",
            self.stage, self.code
        )
    }
}

impl std::error::Error for SandboxError {}

/// Runs `spec` restricted; see the crate docs. `cancel` is polled while the
/// process runs.
pub fn run(spec: &SandboxSpec, cancel: &AtomicBool) -> Result<SandboxRun, SandboxError> {
    #[cfg(windows)]
    {
        windows::run(spec, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = (spec, cancel);
        Err(SandboxError {
            stage: "unsupportedPlatform",
            code: 0,
        })
    }
}

/// The user's profile and roaming AppData folders, as resolved by this (the
/// app's own, unrestricted) process.
///
/// Tectonic resolves its config directory through the `directories` crate,
/// which asks Windows to verify that these two folders exist. Inside an
/// AppContainer that check is denied, so the engine cannot start. Granting
/// exactly these two directory objects `attribute_only_dirs` access is the
/// approved fix (2026-10-30); nothing below them becomes reachable.
pub fn profile_folders() -> Result<Vec<PathBuf>, SandboxError> {
    #[cfg(windows)]
    {
        windows::profile_folders()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// Removes every allow entry for `profile`'s AppContainer from `directory`'s
/// own access list (uninstall, tests). Entries inherited from a parent are not
/// touched. Removing an entry that is not there succeeds.
pub fn revoke(profile: &str, directory: &std::path::Path) -> Result<(), SandboxError> {
    #[cfg(windows)]
    {
        windows::revoke(profile, directory)
    }
    #[cfg(not(windows))]
    {
        let _ = (profile, directory);
        Ok(())
    }
}

/// The AppContainer SID of `profile` in string form (`S-1-15-2-...`), for
/// `icacls ... /remove` or audits. Does not create the profile.
pub fn profile_sid(profile: &str) -> Result<String, SandboxError> {
    #[cfg(windows)]
    {
        windows::profile_sid(profile)
    }
    #[cfg(not(windows))]
    {
        let _ = profile;
        Err(SandboxError {
            stage: "unsupportedPlatform",
            code: 0,
        })
    }
}

/// Removes the AppContainer profile (tests, uninstall). Granted directory
/// permissions stay on directories the app created; call `revoke` first for
/// directories the app did not create (see `profile_folders`).
pub fn delete_profile(profile: &str) -> Result<(), SandboxError> {
    #[cfg(windows)]
    {
        windows::delete_profile(profile)
    }
    #[cfg(not(windows))]
    {
        let _ = profile;
        Ok(())
    }
}
