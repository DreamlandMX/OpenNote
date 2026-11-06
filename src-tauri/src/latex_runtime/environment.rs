//! Preparing, verifying and removing the local LaTeX environment.
//!
//! Layout under the runtime root (an app-created directory):
//!
//! * `active/` — the environment in use: `engine/`, `cache/`, `manifest.json`;
//! * `staging-<operationId>/` — a preparation in progress; never used;
//! * `retired-<operationId>/` — the previous `active/` during the swap.
//!
//! A preparation happens only after the user agreed to the plan, and only in
//! `staging-*`: download and verify the engine, fetch resources in the
//! internet-only `PREPARE_PROFILE`, then compile every template again in the
//! no-network compile profile with the cache **read-only**. Only if that works
//! is the manifest of every file and hash written and the staging directory
//! renamed to `active/`. Cancel, timeout, a limit or any failure deletes the
//! staging directory; nothing half-prepared is ever activated.
//!
//! `verify` re-hashes `active/` against its manifest; any difference (changed,
//! missing or extra file, other engine) means "not prepared".

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use onote_latex_sandbox::{Exit, Network, SandboxRun, SandboxSpec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{archive, download};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSpec {
    pub name: String,
    pub version: String,
    pub license: String,
    pub url: String,
    pub archive_bytes: u64,
    pub archive_sha256: String,
    pub entry: String,
    pub exe_bytes: u64,
    pub exe_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSpec {
    pub source: String,
    pub estimated_bytes: u64,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitSpec {
    pub download_seconds: u64,
    pub prepare_seconds: u64,
}

/// The reviewed, pinned inputs (`resources/latex/runtime-manifest.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedManifest {
    pub format_version: u32,
    pub engine: EngineSpec,
    pub resources: ResourceSpec,
    pub limits: LimitSpec,
}

pub fn fixed_manifest() -> FixedManifest {
    serde_json::from_str(include_str!("../../resources/latex/runtime-manifest.json"))
        .expect("the bundled runtime manifest is valid")
}

/// One file of a prepared environment, relative to `active/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// Written only after the offline check passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedManifest {
    pub format_version: u32,
    pub engine_version: String,
    pub engine_sha256: String,
    pub prepared_at: u64,
    pub total_bytes: u64,
    pub files: Vec<PreparedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum EnvironmentState {
    NotPrepared,
    Ready {
        total_bytes: u64,
        file_count: usize,
    },
    /// Present but not usable; treated exactly like not prepared.
    Invalid {
        reason_key: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Downloading,
    Verifying,
    FetchingResources,
    CheckingOffline,
    Activating,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: Stage,
    pub bytes: u64,
    pub total_bytes: Option<u64>,
    pub step: usize,
    pub steps: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    Cancelled,
    TimedOut,
    TooLarge,
    Download(download::DownloadError),
    EngineMismatch,
    Archive(archive::ArchiveError),
    /// A template did not compile; `log` keeps the engine's tail.
    Engine {
        step: &'static str,
        log: String,
    },
    /// A run left processes behind; nothing from it is trusted.
    Isolation,
    Sandbox(onote_latex_sandbox::SandboxError),
    Io,
}

impl PrepareError {
    pub fn reason_key(&self) -> &'static str {
        match self {
            PrepareError::Cancelled => "latexEnv.reason.cancelled",
            PrepareError::TimedOut => "latexEnv.reason.timedOut",
            PrepareError::TooLarge => "latexEnv.reason.tooLarge",
            PrepareError::Download(_) => "latexEnv.reason.download",
            PrepareError::EngineMismatch | PrepareError::Archive(_) => {
                "latexEnv.reason.engineMismatch"
            }
            PrepareError::Engine { .. } => "latexEnv.reason.engine",
            PrepareError::Isolation | PrepareError::Sandbox(_) => "latexEnv.reason.sandbox",
            PrepareError::Io => "latexEnv.reason.io",
        }
    }
}

/// Where the engine archive comes from: the pinned URL, or (tests) a file.
#[derive(Debug, Clone)]
pub enum EngineSource {
    Download,
    File(PathBuf),
}

/// Everything a preparation needs besides the fixed manifest.
#[derive(Debug, Clone)]
pub struct PrepareRequest {
    pub operation_id: String,
    pub engine: EngineSource,
    /// `(name, files)` per template; each has `main.tex`.
    pub templates: Vec<(String, Vec<(String, String)>)>,
    pub prepare_profile: String,
    pub compile_profile: String,
    pub deadline: Instant,
    pub download_deadline: Instant,
    pub max_resource_bytes: u64,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hash_file(path: &Path) -> std::io::Result<(u64, String)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok((size, hex_digest(hasher.finalize())));
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
    }
}

#[cfg(windows)]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// Every regular file under `base/<prefix>`, relative to `base`. A reparse
/// point is an error: nothing outside the directory may be counted as ours.
fn walk(base: &Path, prefix: &str) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    let mut stack = vec![(base.join(prefix), prefix.to_owned())];
    while let Some((directory, relative)) = stack.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let meta = std::fs::symlink_metadata(entry.path())?;
            if is_reparse(&meta) {
                return Err(std::io::Error::other("reparse point"));
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = format!("{relative}/{name}");
            if meta.is_dir() {
                stack.push((entry.path(), child));
            } else if meta.is_file() {
                found.push((child, entry.path()));
            }
        }
    }
    found.sort();
    Ok(found)
}

pub fn active_dir(root: &Path) -> PathBuf {
    root.join("active")
}

/// Re-hashes `active/` against its manifest.
pub fn verify(root: &Path) -> EnvironmentState {
    let active = active_dir(root);
    let manifest_path = active.join("manifest.json");
    let bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return EnvironmentState::NotPrepared
        }
        Err(_) => {
            return EnvironmentState::Invalid {
                reason_key: "latexEnv.invalid.unreadable",
            }
        }
    };
    let Ok(manifest) = serde_json::from_slice::<PreparedManifest>(&bytes) else {
        return EnvironmentState::Invalid {
            reason_key: "latexEnv.invalid.unreadable",
        };
    };
    let fixed = fixed_manifest();
    if manifest.format_version != 1
        || manifest.engine_sha256 != fixed.engine.exe_sha256
        || manifest.engine_version != fixed.engine.version
    {
        return EnvironmentState::Invalid {
            reason_key: "latexEnv.invalid.engine",
        };
    }
    let mut on_disk = Vec::new();
    for prefix in ["engine", "cache"] {
        match walk(&active, prefix) {
            Ok(files) => on_disk.extend(files),
            Err(_) => {
                return EnvironmentState::Invalid {
                    reason_key: "latexEnv.invalid.unreadable",
                }
            }
        }
    }
    on_disk.sort();
    if on_disk.len() != manifest.files.len()
        || on_disk
            .iter()
            .zip(&manifest.files)
            .any(|((path, _), recorded)| *path != recorded.path)
    {
        return EnvironmentState::Invalid {
            reason_key: "latexEnv.invalid.files",
        };
    }
    for ((_, path), recorded) in on_disk.iter().zip(&manifest.files) {
        match hash_file(path) {
            Ok((size, sha)) if size == recorded.size && sha == recorded.sha256 => {}
            _ => {
                return EnvironmentState::Invalid {
                    reason_key: "latexEnv.invalid.files",
                }
            }
        }
    }
    EnvironmentState::Ready {
        total_bytes: manifest.total_bytes,
        file_count: manifest.files.len(),
    }
}

/// Removes leftovers of an interrupted preparation or swap.
pub fn clean_leftovers(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("staging-") || name.starts_with("retired-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn set_progress(progress: &Mutex<Progress>, update: impl FnOnce(&mut Progress)) {
    if let Ok(mut value) = progress.lock() {
        update(&mut value);
    }
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let start = text.len().saturating_sub(4000);
    let start = (start..=text.len())
        .find(|at| text.is_char_boundary(*at))
        .unwrap_or(0);
    text[start..].to_owned()
}

struct EngineRun<'a> {
    profile: &'a str,
    network: Network,
    engine: &'a Path,
    input: &'a Path,
    output: &'a Path,
    cache: &'a Path,
    cache_writable: bool,
    budget: Option<u64>,
}

fn run_engine(
    run: EngineRun<'_>,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<SandboxRun, PrepareError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(PrepareError::TimedOut)?;
    let mut args: Vec<std::ffi::OsString> = vec!["-X".into(), "compile".into()];
    if run.network == Network::None {
        args.push("--only-cached".into());
    }
    args.extend([
        "--untrusted".into(),
        "--synctex".into(),
        "--outdir".into(),
        run.output.as_os_str().to_owned(),
        run.input.join("main.tex").into_os_string(),
    ]);
    let mut read_dirs = vec![
        run.engine.parent().unwrap_or(run.engine).to_owned(),
        run.input.to_owned(),
    ];
    let mut write_dirs = vec![run.output.to_owned()];
    if run.cache_writable {
        write_dirs.push(run.cache.to_owned());
    } else {
        read_dirs.push(run.cache.to_owned());
    }
    let spec = SandboxSpec {
        profile: run.profile.to_owned(),
        program: run.engine.to_owned(),
        args,
        cwd: run.output.to_owned(),
        read_dirs,
        write_dirs,
        attribute_only_dirs: onote_latex_sandbox::profile_folders()
            .map_err(PrepareError::Sandbox)?,
        timeout: remaining,
        max_output_bytes: 1024 * 1024,
        env: vec![(
            "TECTONIC_CACHE_DIR".to_owned(),
            run.cache.as_os_str().to_owned(),
        )],
        network: run.network,
        byte_budget: run.budget.map(|limit| (run.cache.to_owned(), limit)),
    };
    let result = onote_latex_sandbox::run(&spec, cancel).map_err(PrepareError::Sandbox)?;
    if result.remaining_processes != 0 {
        return Err(PrepareError::Isolation);
    }
    match result.exit {
        Exit::Exited(_) => Ok(result),
        Exit::Cancelled => Err(PrepareError::Cancelled),
        Exit::TimedOut => Err(PrepareError::TimedOut),
        Exit::ByteBudgetExceeded => Err(PrepareError::TooLarge),
    }
}

fn compiled(result: &SandboxRun, output: &Path) -> bool {
    matches!(result.exit, Exit::Exited(0))
        && std::fs::read(output.join("main.pdf"))
            .map(|pdf| pdf.starts_with(b"%PDF-"))
            .unwrap_or(false)
}

fn engine_archive(
    request: &PrepareRequest,
    fixed: &FixedManifest,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> Result<Vec<u8>, PrepareError> {
    match &request.engine {
        EngineSource::Download => download::download(
            &fixed.engine.url,
            fixed.engine.archive_bytes,
            request.download_deadline.min(request.deadline),
            cancel,
            |bytes| set_progress(progress, |value| value.bytes = bytes),
        )
        .map_err(|error| match error {
            download::DownloadError::Cancelled => PrepareError::Cancelled,
            download::DownloadError::TimedOut => PrepareError::TimedOut,
            download::DownloadError::TooLarge => PrepareError::TooLarge,
            other => PrepareError::Download(other),
        }),
        EngineSource::File(path) => std::fs::read(path).map_err(|_| PrepareError::Io),
    }
}

/// The preparation steps inside `staging`; see the module docs.
fn prepare_in(
    root: &Path,
    staging: &Path,
    request: &PrepareRequest,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> Result<PreparedManifest, PrepareError> {
    let fixed = fixed_manifest();
    let io = |_| PrepareError::Io;
    let [engine_dir, cache, input, work, offline] =
        ["engine", "cache", "input", "work", "offline"].map(|name| staging.join(name));
    for directory in [&engine_dir, &cache, &input, &work, &offline] {
        std::fs::create_dir_all(directory).map_err(io)?;
    }

    set_progress(progress, |value| {
        value.stage = Stage::Downloading;
        value.total_bytes = Some(fixed.engine.archive_bytes);
    });
    let archive_bytes = engine_archive(request, &fixed, cancel, progress)?;
    set_progress(progress, |value| value.stage = Stage::Verifying);
    if archive_bytes.len() as u64 != fixed.engine.archive_bytes
        || sha256_hex(&archive_bytes) != fixed.engine.archive_sha256
    {
        return Err(PrepareError::EngineMismatch);
    }
    let exe = archive::extract_single(&archive_bytes, &fixed.engine.entry, fixed.engine.exe_bytes)
        .map_err(PrepareError::Archive)?;
    drop(archive_bytes);
    if sha256_hex(&exe) != fixed.engine.exe_sha256 {
        return Err(PrepareError::EngineMismatch);
    }
    let engine = engine_dir.join(&fixed.engine.entry);
    std::fs::write(&engine, &exe).map_err(io)?;
    drop(exe);
    if hash_file(&engine).map_err(io)?.1 != fixed.engine.exe_sha256 {
        return Err(PrepareError::EngineMismatch);
    }

    for (name, files) in &request.templates {
        for (path, content) in files {
            let target = input.join(name).join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            std::fs::write(target, content).map_err(io)?;
        }
    }

    let steps = request.templates.len();
    for (index, (name, _)) in request.templates.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(PrepareError::Cancelled);
        }
        set_progress(progress, |value| {
            value.stage = Stage::FetchingResources;
            value.step = index + 1;
            value.steps = steps;
            value.total_bytes = Some(request.max_resource_bytes);
        });
        let output = work.join(name);
        std::fs::create_dir_all(&output).map_err(io)?;
        let result = run_engine(
            EngineRun {
                profile: &request.prepare_profile,
                network: Network::InternetClientOnly,
                engine: &engine,
                input: &input.join(name),
                output: &output,
                cache: &cache,
                cache_writable: true,
                budget: Some(request.max_resource_bytes),
            },
            request.deadline,
            cancel,
        )?;
        if !compiled(&result, &output) {
            return Err(PrepareError::Engine {
                step: "fetch",
                log: tail(&result.stderr),
            });
        }
        let cached: u64 = walk(staging, "cache")
            .map_err(io)?
            .iter()
            .filter_map(|(_, path)| std::fs::metadata(path).ok().map(|meta| meta.len()))
            .sum();
        set_progress(progress, |value| value.bytes = cached);
    }

    // The proof that the environment works offline: no network, cache read-only.
    for (index, (name, _)) in request.templates.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(PrepareError::Cancelled);
        }
        set_progress(progress, |value| {
            value.stage = Stage::CheckingOffline;
            value.step = index + 1;
            value.steps = steps;
        });
        let output = offline.join(name);
        std::fs::create_dir_all(&output).map_err(io)?;
        let result = run_engine(
            EngineRun {
                profile: &request.compile_profile,
                network: Network::None,
                engine: &engine,
                input: &input.join(name),
                output: &output,
                cache: &cache,
                cache_writable: false,
                budget: None,
            },
            request.deadline,
            cancel,
        )?;
        if !compiled(&result, &output) {
            return Err(PrepareError::Engine {
                step: "offline",
                log: tail(&result.stderr),
            });
        }
    }

    set_progress(progress, |value| value.stage = Stage::Activating);
    for directory in [&input, &work, &offline] {
        std::fs::remove_dir_all(directory).map_err(io)?;
    }
    let mut files = Vec::new();
    let mut total = 0u64;
    for prefix in ["engine", "cache"] {
        for (path, absolute) in walk(staging, prefix).map_err(io)? {
            let (size, sha256) = hash_file(&absolute).map_err(io)?;
            total += size;
            files.push(PreparedFile { path, size, sha256 });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = PreparedManifest {
        format_version: 1,
        engine_version: fixed.engine.version.clone(),
        engine_sha256: fixed.engine.exe_sha256.clone(),
        prepared_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0),
        total_bytes: total,
        files,
    };
    let json = serde_json::to_vec_pretty(&manifest).map_err(|_| PrepareError::Io)?;
    std::fs::write(staging.join("manifest.json"), json).map_err(io)?;
    activate(root, staging, &request.operation_id)?;
    Ok(manifest)
}

/// Swaps `staging` in as `active/`, keeping the previous one until it worked.
fn activate(root: &Path, staging: &Path, operation_id: &str) -> Result<(), PrepareError> {
    let active = active_dir(root);
    let retired = root.join(format!("retired-{operation_id}"));
    let had_active = active.exists();
    if had_active {
        std::fs::rename(&active, &retired).map_err(|_| PrepareError::Io)?;
    }
    if std::fs::rename(staging, &active).is_err() {
        if had_active {
            let _ = std::fs::rename(&retired, &active);
        }
        return Err(PrepareError::Io);
    }
    if had_active {
        let _ = std::fs::remove_dir_all(&retired);
    }
    Ok(())
}

/// Runs a whole preparation. On any error the staging directory is deleted,
/// so the previous `active/` (if any) stays exactly as it was.
pub fn prepare(
    root: &Path,
    request: &PrepareRequest,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> Result<PreparedManifest, PrepareError> {
    std::fs::create_dir_all(root).map_err(|_| PrepareError::Io)?;
    clean_leftovers(root);
    let staging = root.join(format!("staging-{}", request.operation_id));
    let outcome = prepare_in(root, &staging, request, cancel, progress);
    if outcome.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    outcome
}

/// The default deadlines from the fixed manifest.
pub fn deadlines(fixed: &FixedManifest) -> (Instant, Instant) {
    let now = Instant::now();
    (
        now + Duration::from_secs(fixed.limits.prepare_seconds),
        now + Duration::from_secs(fixed.limits.download_seconds),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_manifest_pins_https_sources_and_bounded_limits() {
        let fixed = fixed_manifest();
        assert!(fixed
            .engine
            .url
            .starts_with("https://github.com/tectonic-typesetting/"));
        assert_eq!(fixed.engine.archive_sha256.len(), 64);
        assert_eq!(fixed.engine.exe_sha256.len(), 64);
        assert!(fixed.resources.max_bytes >= fixed.resources.estimated_bytes);
        assert!(fixed.resources.max_bytes <= 512 * 1024 * 1024);
        assert!(fixed.limits.prepare_seconds <= 3600);
    }

    fn write_active(root: &Path, files: &[(&str, &[u8])]) {
        let active = active_dir(root);
        let mut recorded = Vec::new();
        for (path, bytes) in files {
            let target = active.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, bytes).unwrap();
            recorded.push(PreparedFile {
                path: (*path).to_owned(),
                size: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            });
        }
        recorded.sort_by(|left, right| left.path.cmp(&right.path));
        let fixed = fixed_manifest();
        let manifest = PreparedManifest {
            format_version: 1,
            engine_version: fixed.engine.version,
            engine_sha256: fixed.engine.exe_sha256,
            prepared_at: 0,
            total_bytes: files.iter().map(|(_, bytes)| bytes.len() as u64).sum(),
            files: recorded,
        };
        std::fs::write(
            active.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    const FILES: &[(&str, &[u8])] = &[
        ("engine/tectonic.exe", b"engine"),
        ("cache/formats/latex.fmt", b"format"),
        ("cache/files/ab/cd", b"package"),
    ];

    #[test]
    fn an_intact_environment_is_ready() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(verify(root.path()), EnvironmentState::NotPrepared);
        write_active(root.path(), FILES);
        assert_eq!(
            verify(root.path()),
            EnvironmentState::Ready {
                total_bytes: 19,
                file_count: 3
            }
        );
    }

    #[test]
    fn any_change_to_the_prepared_files_makes_it_unprepared() {
        let changed = |mutate: &dyn Fn(&Path)| {
            let root = tempfile::tempdir().unwrap();
            write_active(root.path(), FILES);
            mutate(&active_dir(root.path()));
            verify(root.path())
        };
        let invalid = EnvironmentState::Invalid {
            reason_key: "latexEnv.invalid.files",
        };
        assert_eq!(
            changed(
                &|active| std::fs::write(active.join("cache/files/ab/cd"), b"packagE").unwrap()
            ),
            invalid
        );
        assert_eq!(
            changed(
                &|active| std::fs::remove_file(active.join("cache/formats/latex.fmt")).unwrap()
            ),
            invalid
        );
        assert_eq!(
            changed(&|active| std::fs::write(active.join("cache/extra"), b"x").unwrap()),
            invalid
        );
        assert_eq!(
            changed(&|active| std::fs::write(active.join("manifest.json"), b"{").unwrap()),
            EnvironmentState::Invalid {
                reason_key: "latexEnv.invalid.unreadable"
            }
        );
    }

    #[test]
    fn a_manifest_for_another_engine_is_not_used() {
        let root = tempfile::tempdir().unwrap();
        write_active(root.path(), FILES);
        let path = active_dir(root.path()).join("manifest.json");
        let mut manifest: PreparedManifest =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest.engine_sha256 = "0".repeat(64);
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(
            verify(root.path()),
            EnvironmentState::Invalid {
                reason_key: "latexEnv.invalid.engine"
            }
        );
    }

    fn request(root: &Path, engine: EngineSource) -> PrepareRequest {
        let _ = root;
        PrepareRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            engine,
            templates: vec![(
                "t".to_owned(),
                vec![("main.tex".to_owned(), "x".to_owned())],
            )],
            prepare_profile: "unused".to_owned(),
            compile_profile: "unused".to_owned(),
            deadline: Instant::now() + Duration::from_secs(60),
            download_deadline: Instant::now() + Duration::from_secs(60),
            max_resource_bytes: 1024,
        }
    }

    fn idle() -> Mutex<Progress> {
        Mutex::new(Progress {
            stage: Stage::Downloading,
            bytes: 0,
            total_bytes: None,
            step: 0,
            steps: 0,
        })
    }

    #[test]
    fn a_wrong_engine_archive_activates_nothing_and_leaves_no_staging() {
        let root = tempfile::tempdir().unwrap();
        write_active(root.path(), FILES);
        let fake = root.path().join("fake.zip");
        std::fs::write(
            &fake,
            archive::tests::zip(&[("tectonic.exe", b"not the engine", true)]),
        )
        .unwrap();
        let outcome = prepare(
            root.path(),
            &request(root.path(), EngineSource::File(fake)),
            &AtomicBool::new(false),
            &idle(),
        );
        assert_eq!(outcome.unwrap_err(), PrepareError::EngineMismatch);
        // The previous environment is untouched and still verifies.
        assert!(matches!(
            verify(root.path()),
            EnvironmentState::Ready { .. }
        ));
        let leftovers: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("staging-") || name.starts_with("retired-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_cancelled_preparation_activates_nothing() {
        let root = tempfile::tempdir().unwrap();
        let outcome = prepare(
            root.path(),
            &request(root.path(), EngineSource::Download),
            &AtomicBool::new(true),
            &idle(),
        );
        assert!(outcome.is_err());
        assert_eq!(verify(root.path()), EnvironmentState::NotPrepared);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn leftovers_of_an_interrupted_run_are_removed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("staging-x/cache")).unwrap();
        std::fs::create_dir_all(root.path().join("retired-y")).unwrap();
        write_active(root.path(), FILES);
        clean_leftovers(root.path());
        let names: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["active".to_owned()]);
    }
}
