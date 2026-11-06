//! F51–F53 one real build: the engine in the no-network sandbox, the PDF check
//! in its own sandboxed process, and the atomic publication of the result.
//!
//! Layout under the builds root (an app-created directory, outside the vault):
//!
//! * `validator/<sha>/` — a copy of this program used for `--latex-validate-pdf`
//!   (the sandbox may only read app-created directories, not the install one);
//! * `<project>/jobs/<jobId>/` — `input/` (the snapshot), `output/` (engine),
//!   `check/` (the copy being validated), `job.json`;
//! * `<project>/artifacts/<artifactId>/` — a published, validated PDF with
//!   `artifact.json`; `<project>/latest.json` names the current one.
//!
//! Nothing the engine wrote is trusted until it passed the check, and the
//! published bytes are the exact bytes that were checked (hash compared).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use onote_latex_sandbox::{Exit, Network, SandboxSpec};
use serde::{Deserialize, Serialize};

use super::environment::{self, sha256_hex};
use crate::pdf_validate::{self, Verdict};

/// F51 default: engine and bibliography together.
pub const COMPILE_TIMEOUT: Duration = Duration::from_secs(120);
/// stdout and stderr are each kept up to this, so 8 MiB of log in total.
pub const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;
/// The engine's own `.log` is read up to this (head and tail when larger).
pub const MAX_LOG_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Everything the engine may write: the PDF bound plus logs and SyncTeX.
pub const MAX_OUTPUT_BYTES: u64 = pdf_validate::MAX_PDF_BYTES + 64 * 1024 * 1024;
/// One chunk of artifact bytes over IPC.
pub const MAX_CHUNK_BYTES: u64 = 8 * 1024 * 1024;
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(60);

/// Why a build produced no published PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    Cancelled,
    TimedOut,
    /// The engine ran and failed; its log explains why.
    EngineFailed,
    NoPdf,
    TooLarge,
    InvalidPdf(String),
    /// Processes were left behind, or the sandbox could not be set up.
    Sandbox,
    /// The prepared environment is gone or failed its check.
    EnvironmentMissing,
    Io,
}

impl BuildError {
    pub fn reason_key(&self) -> &'static str {
        match self {
            BuildError::Cancelled => "build.cancelled",
            BuildError::TimedOut => "build.timedOut",
            BuildError::EngineFailed => "build.failed",
            BuildError::NoPdf => "build.noPdf",
            BuildError::TooLarge => "build.tooLarge",
            BuildError::InvalidPdf(_) => "build.invalidPdf",
            BuildError::Sandbox => "build.sandbox",
            BuildError::EnvironmentMissing => "build.environmentMissing",
            BuildError::Io => "build.io",
        }
    }
}

fn io(_: std::io::Error) -> BuildError {
    BuildError::Io
}

/// What the engine left: its exit, bounded logs and the candidate outputs.
#[derive(Debug, Clone)]
pub struct EngineOutcome {
    pub exit_code: Option<u32>,
    /// The engine's `.log` (bounded), or empty when it wrote none.
    pub log: String,
    pub stderr: String,
    pub truncated: bool,
    pub pdf: Option<PathBuf>,
    pub synctex: Option<PathBuf>,
}

pub struct EngineJob<'a> {
    pub profile: &'a str,
    /// The verified `active/` environment.
    pub active: &'a Path,
    pub input: &'a Path,
    /// Entry file relative to `input`, with `/` separators.
    pub entry: &'a str,
    pub output: &'a Path,
    pub timeout: Duration,
}

fn native(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

fn stem(entry: &str) -> &str {
    let name = entry.rsplit('/').next().unwrap_or(entry);
    name.strip_suffix(".tex").unwrap_or(name)
}

/// Reads at most `MAX_LOG_FILE_BYTES`: head and tail when larger.
fn bounded_text(path: &Path) -> (String, bool) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return (String::new(), false);
    };
    let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    if size <= MAX_LOG_FILE_BYTES {
        let mut bytes = Vec::new();
        let _ = file.read_to_end(&mut bytes);
        return (String::from_utf8_lossy(&bytes).into_owned(), false);
    }
    let half = (MAX_LOG_FILE_BYTES / 2) as usize;
    let mut head = vec![0u8; half];
    let _ = file.read_exact(&mut head);
    use std::io::Seek;
    let _ = file.seek(std::io::SeekFrom::End(-(half as i64)));
    let mut tail = vec![0u8; half];
    let _ = file.read_exact(&mut tail);
    let text = format!(
        "{}\n[… log truncated …]\n{}",
        String::from_utf8_lossy(&head),
        String::from_utf8_lossy(&tail)
    );
    (text, true)
}

/// Compiles the snapshot in `input` with no network and the environment
/// read-only. Only `output` is writable, and its size is bounded.
pub fn run_engine(job: &EngineJob<'_>, cancel: &AtomicBool) -> Result<EngineOutcome, BuildError> {
    let fixed = environment::fixed_manifest();
    let engine = job.active.join("engine").join(&fixed.engine.entry);
    let cache = job.active.join("cache");
    let args: Vec<std::ffi::OsString> = vec![
        "-X".into(),
        "compile".into(),
        "--only-cached".into(),
        "--untrusted".into(),
        "--synctex".into(),
        "--keep-logs".into(),
        "--outdir".into(),
        job.output.as_os_str().to_owned(),
        native(job.input, job.entry).into_os_string(),
    ];
    let spec = SandboxSpec {
        profile: job.profile.to_owned(),
        program: engine.clone(),
        args,
        cwd: job.output.to_owned(),
        read_dirs: vec![
            job.active.join("engine"),
            cache.clone(),
            job.input.to_owned(),
        ],
        write_dirs: vec![job.output.to_owned()],
        attribute_only_dirs: onote_latex_sandbox::profile_folders()
            .map_err(|_| BuildError::Sandbox)?,
        timeout: job.timeout,
        max_output_bytes: MAX_STREAM_BYTES,
        env: vec![("TECTONIC_CACHE_DIR".to_owned(), cache.into_os_string())],
        network: Network::None,
        byte_budget: Some((job.output.to_owned(), MAX_OUTPUT_BYTES)),
    };
    let run = onote_latex_sandbox::run(&spec, cancel).map_err(|_| BuildError::Sandbox)?;
    if run.remaining_processes != 0 {
        return Err(BuildError::Sandbox);
    }
    let exit_code = match run.exit {
        Exit::Exited(code) => code,
        Exit::Cancelled => return Err(BuildError::Cancelled),
        Exit::TimedOut => return Err(BuildError::TimedOut),
        Exit::ByteBudgetExceeded => return Err(BuildError::TooLarge),
    };
    let stem = stem(job.entry);
    let (log, log_truncated) = bounded_text(&job.output.join(format!("{stem}.log")));
    let existing = |name: String| {
        let path = job.output.join(name);
        std::fs::symlink_metadata(&path)
            .is_ok_and(|meta| meta.is_file())
            .then_some(path)
    };
    Ok(EngineOutcome {
        exit_code: Some(exit_code),
        log,
        stderr: String::from_utf8_lossy(&run.stderr).into_owned(),
        truncated: run.truncated || log_truncated,
        pdf: existing(format!("{stem}.pdf")),
        synctex: existing(format!("{stem}.synctex.gz")),
    })
}

/// A copy of `source` (this program) in an app-created directory the sandbox
/// may read, verified by hash. `vcruntime140.dll` is copied beside it because
/// the system copy is not readable from an AppContainer on every machine.
pub fn validator_copy(builds_root: &Path, source: &Path) -> Result<PathBuf, BuildError> {
    let bytes = std::fs::read(source).map_err(io)?;
    let sha = sha256_hex(&bytes);
    let directory = builds_root.join("validator").join(&sha[..32]);
    let target = directory.join("opennote-validate.exe");
    let intact = std::fs::read(&target).is_ok_and(|copy| sha256_hex(&copy) == sha);
    if !intact {
        let staging = builds_root
            .join("validator")
            .join(format!(".staging-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&staging).map_err(io)?;
        std::fs::write(staging.join("opennote-validate.exe"), &bytes).map_err(io)?;
        if let Some(system) = std::env::var_os("SystemRoot") {
            let runtime = PathBuf::from(system)
                .join("System32")
                .join("vcruntime140.dll");
            if runtime.is_file() {
                std::fs::copy(&runtime, staging.join("vcruntime140.dll")).map_err(io)?;
            }
        }
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::rename(&staging, &directory).map_err(io)?;
        let copy = std::fs::read(&target).map_err(io)?;
        if sha256_hex(&copy) != sha {
            return Err(BuildError::Io);
        }
    }
    // Older copies belong to builds of OpenNote that are gone.
    if let Ok(entries) = std::fs::read_dir(builds_root.join("validator")) {
        for entry in entries.flatten() {
            if entry.path() != directory {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(target)
}

/// Runs the structural check of `pdf` (inside `check`, a directory holding
/// only that file) in the sandbox; read-only access to `check`, nothing else.
pub fn validate(
    validator: &Path,
    profile: &str,
    check: &Path,
    pdf: &Path,
    scratch: &Path,
    cancel: &AtomicBool,
) -> Result<u32, BuildError> {
    let spec = SandboxSpec {
        profile: profile.to_owned(),
        program: validator.to_owned(),
        args: vec![pdf_validate::FLAG.into(), pdf.as_os_str().to_owned()],
        cwd: scratch.to_owned(),
        read_dirs: vec![
            validator.parent().unwrap_or(validator).to_owned(),
            check.to_owned(),
        ],
        write_dirs: vec![scratch.to_owned()],
        attribute_only_dirs: Vec::new(),
        timeout: VALIDATE_TIMEOUT,
        max_output_bytes: 64 * 1024,
        env: Vec::new(),
        network: Network::None,
        byte_budget: Some((scratch.to_owned(), 1024 * 1024)),
    };
    let run = onote_latex_sandbox::run(&spec, cancel).map_err(|_| BuildError::Sandbox)?;
    if run.remaining_processes != 0 {
        return Err(BuildError::Sandbox);
    }
    match run.exit {
        Exit::Cancelled => return Err(BuildError::Cancelled),
        Exit::TimedOut => return Err(BuildError::InvalidPdf("timedOut".to_owned())),
        Exit::ByteBudgetExceeded => return Err(BuildError::InvalidPdf("wrote".to_owned())),
        Exit::Exited(_) => {}
    }
    let line = String::from_utf8_lossy(&run.stdout);
    match serde_json::from_str::<Verdict>(line.trim()) {
        Ok(Verdict::Valid { pages }) if run.exit == Exit::Exited(0) => Ok(pages),
        Ok(Verdict::Invalid { reason }) => Err(BuildError::InvalidPdf(reason)),
        // A crash or anything unexpected is a refusal, never a pass.
        _ => Err(BuildError::InvalidPdf("crashed".to_owned())),
    }
}

/// A published PDF; `artifact.json` beside the bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub format_version: u32,
    pub artifact_id: String,
    pub project_id: String,
    pub job_id: String,
    /// The project manifest the snapshot was taken from.
    pub manifest_hash: String,
    /// Shown in the reader, e.g. `main.pdf`.
    pub label: String,
    pub bytes: u64,
    pub sha256: String,
    pub pages: u32,
    pub synctex: bool,
    pub built_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Latest {
    artifact_id: String,
}

pub(crate) fn artifact_dir(project_dir: &Path, artifact_id: &str) -> PathBuf {
    project_dir.join("artifacts").join(artifact_id)
}

fn hash_file(path: &Path) -> std::io::Result<(u64, String)> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok((size, format!("{:x}", hasher.finalize())));
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
    }
}

/// What a build wants to publish: the checked PDF and its identity.
pub struct Publication<'a> {
    pub project_id: &'a str,
    pub job_id: &'a str,
    pub manifest_hash: &'a str,
    pub label: &'a str,
    /// The checked copy and the hash it had when it was checked.
    pub pdf: &'a Path,
    pub checked_sha256: &'a str,
    pub pages: u32,
    pub synctex: Option<&'a Path>,
}

/// Publishes atomically: the artifact directory is complete before it gets its
/// name, and `latest.json` is replaced in one rename. Until then the previous
/// artifact stays current; older artifacts are removed only afterwards.
pub fn publish(project_dir: &Path, item: &Publication<'_>) -> Result<ArtifactRecord, BuildError> {
    let artifact_id = uuid::Uuid::new_v4().to_string();
    let artifacts = project_dir.join("artifacts");
    let staging = artifacts.join(format!(".staging-{artifact_id}"));
    let outcome = (|| {
        std::fs::create_dir_all(&staging).map_err(io)?;
        let target = staging.join("document.pdf");
        std::fs::copy(item.pdf, &target).map_err(io)?;
        let (bytes, sha256) = hash_file(&target).map_err(io)?;
        if sha256 != item.checked_sha256 {
            // Changed after the check: publish nothing rather than unchecked bytes.
            return Err(BuildError::InvalidPdf("changed".to_owned()));
        }
        if let Some(synctex) = item.synctex {
            std::fs::copy(synctex, staging.join("document.synctex.gz")).map_err(io)?;
        }
        let record = ArtifactRecord {
            format_version: 1,
            artifact_id: artifact_id.clone(),
            project_id: item.project_id.to_owned(),
            job_id: item.job_id.to_owned(),
            manifest_hash: item.manifest_hash.to_owned(),
            label: item.label.to_owned(),
            bytes,
            sha256,
            pages: item.pages,
            synctex: item.synctex.is_some(),
            built_at: now_secs(),
        };
        let json = serde_json::to_vec_pretty(&record).map_err(|_| BuildError::Io)?;
        std::fs::write(staging.join("artifact.json"), json).map_err(io)?;
        std::fs::rename(&staging, artifact_dir(project_dir, &artifact_id)).map_err(io)?;
        Ok(record)
    })();
    let record = match outcome {
        Ok(record) => record,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    let pointer = project_dir.join(format!("latest.json.{artifact_id}"));
    let json = serde_json::to_vec(&Latest {
        artifact_id: artifact_id.clone(),
    })
    .map_err(|_| BuildError::Io)?;
    let swapped = std::fs::write(&pointer, json)
        .and_then(|()| std::fs::rename(&pointer, project_dir.join("latest.json")));
    if swapped.is_err() {
        let _ = std::fs::remove_file(&pointer);
        let _ = std::fs::remove_dir_all(artifact_dir(project_dir, &artifact_id));
        return Err(BuildError::Io);
    }
    if let Ok(entries) = std::fs::read_dir(&artifacts) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy() != artifact_id {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(record)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The current artifact, re-verified against its record: size and hash of the
/// bytes on disk must match, or there is no usable artifact.
pub fn current_artifact(project_dir: &Path) -> Option<ArtifactRecord> {
    let latest: Latest =
        serde_json::from_slice(&std::fs::read(project_dir.join("latest.json")).ok()?).ok()?;
    uuid::Uuid::parse_str(&latest.artifact_id).ok()?;
    let directory = artifact_dir(project_dir, &latest.artifact_id);
    let record: ArtifactRecord =
        serde_json::from_slice(&std::fs::read(directory.join("artifact.json")).ok()?).ok()?;
    if record.artifact_id != latest.artifact_id || record.format_version != 1 {
        return None;
    }
    let (bytes, sha) = hash_file(&directory.join("document.pdf")).ok()?;
    (bytes == record.bytes && sha == record.sha256).then_some(record)
}

/// One bounded chunk of the current artifact's PDF. Only the artifact named by
/// `latest.json` is readable, and only within its recorded size.
pub fn read_chunk(
    project_dir: &Path,
    artifact_id: &str,
    offset: u64,
    length: u64,
) -> Result<Vec<u8>, BuildError> {
    use std::io::Seek;
    let latest: Latest =
        serde_json::from_slice(&std::fs::read(project_dir.join("latest.json")).map_err(io)?)
            .map_err(|_| BuildError::Io)?;
    if latest.artifact_id != artifact_id {
        return Err(BuildError::NoPdf);
    }
    let directory = artifact_dir(project_dir, artifact_id);
    let record: ArtifactRecord =
        serde_json::from_slice(&std::fs::read(directory.join("artifact.json")).map_err(io)?)
            .map_err(|_| BuildError::Io)?;
    if offset > record.bytes {
        return Err(BuildError::NoPdf);
    }
    let length = length.min(MAX_CHUNK_BYTES).min(record.bytes - offset);
    let mut file = std::fs::File::open(directory.join("document.pdf")).map_err(io)?;
    file.seek(std::io::SeekFrom::Start(offset)).map_err(io)?;
    let mut buffer = vec![0u8; length as usize];
    file.read_exact(&mut buffer).map_err(io)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish_bytes(
        project: &Path,
        bytes: &[u8],
        manifest: &str,
    ) -> Result<ArtifactRecord, BuildError> {
        let source = project.join(format!("candidate-{}.pdf", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(project).unwrap();
        std::fs::write(&source, bytes).unwrap();
        publish(
            project,
            &Publication {
                project_id: "p",
                job_id: "j",
                manifest_hash: manifest,
                label: "main.pdf",
                pdf: &source,
                checked_sha256: &sha256_hex(bytes),
                pages: 1,
                synctex: None,
            },
        )
    }

    #[test]
    fn publishing_replaces_the_current_artifact_atomically_and_removes_the_old_one() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("p");
        assert_eq!(current_artifact(&project), None);
        let first = publish_bytes(&project, b"%PDF-first", "m1").unwrap();
        assert_eq!(current_artifact(&project), Some(first.clone()));
        let second = publish_bytes(&project, b"%PDF-second", "m2").unwrap();
        assert_eq!(
            current_artifact(&project).unwrap().artifact_id,
            second.artifact_id
        );
        assert!(!artifact_dir(&project, &first.artifact_id).exists());
        let names: Vec<_> = std::fs::read_dir(project.join("artifacts"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![second.artifact_id.clone()]);
    }

    #[test]
    fn bytes_changed_after_the_check_are_never_published() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("p");
        let kept = publish_bytes(&project, b"%PDF-kept", "m1").unwrap();
        let source = project.join("candidate.pdf");
        std::fs::write(&source, b"%PDF-swapped").unwrap();
        let outcome = publish(
            &project,
            &Publication {
                project_id: "p",
                job_id: "j2",
                manifest_hash: "m2",
                label: "main.pdf",
                pdf: &source,
                checked_sha256: &sha256_hex(b"%PDF-checked"),
                pages: 1,
                synctex: None,
            },
        );
        assert_eq!(
            outcome.unwrap_err(),
            BuildError::InvalidPdf("changed".to_owned())
        );
        assert_eq!(current_artifact(&project), Some(kept));
    }

    #[test]
    fn a_tampered_artifact_is_not_current_and_reads_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("p");
        let record = publish_bytes(&project, b"%PDF-0123456789", "m").unwrap();
        let id = &record.artifact_id;
        assert_eq!(read_chunk(&project, id, 5, 4).unwrap(), b"0123");
        // The end is clamped to the recorded size.
        assert_eq!(read_chunk(&project, id, 13, 100).unwrap(), b"89");
        assert!(read_chunk(&project, id, 16, 1).is_err());
        // Only the current artifact can be read.
        assert!(read_chunk(&project, &uuid::Uuid::new_v4().to_string(), 0, 1).is_err());
        std::fs::write(
            artifact_dir(&project, id).join("document.pdf"),
            b"%PDF-tampered!!",
        )
        .unwrap();
        assert_eq!(current_artifact(&project), None);
    }

    #[test]
    fn the_engine_log_is_bounded_with_head_and_tail() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.log");
        let mut text = "HEAD\n".to_owned();
        text.push_str(&"x".repeat(MAX_LOG_FILE_BYTES as usize));
        text.push_str("\nTAIL");
        std::fs::write(&path, &text).unwrap();
        let (log, truncated) = bounded_text(&path);
        assert!(truncated);
        assert!(log.starts_with("HEAD"));
        assert!(log.ends_with("TAIL"));
        assert!(log.len() as u64 <= MAX_LOG_FILE_BYTES + 64);
    }

    #[test]
    fn the_entry_stem_names_the_outputs() {
        assert_eq!(stem("main.tex"), "main");
        assert_eq!(stem("src/thesis.tex"), "thesis");
    }
}
