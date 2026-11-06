//! F51–F53 real compiles of registered LaTeX projects.
//!
//! The frontend asks for a build with the manifest hash it was shown; the
//! host takes the snapshot itself, runs the engine (no network, environment
//! read-only), checks the PDF in a separate sandboxed process and publishes it
//! atomically. Every job state is the host's observation: there is no command
//! through which a caller reports a result.
//!
//! One engine runs at a time in the whole app. Per project, one job runs and
//! at most one waits; a newer request replaces the waiting one (M13 `Queue`).
//! Job state is written to disk, so a job found unfinished after a restart is
//! reported as interrupted, never as succeeded.

use super::*;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::latex_project::{capture_snapshot, ProjectSnapshot};
use crate::latex_runtime::compile::{self, ArtifactRecord, BuildError, EngineJob, Publication};
use onote_latex::{JobState, Queue};

/// Persisted job records kept per project.
const KEEP_JOBS: usize = 10;

/// Held for the whole engine + check of one job: one build at a time.
static ENGINE_SLOT: Mutex<()> = Mutex::new(());

/// True while a build is running; the environment must not change under it.
pub(crate) fn engine_busy() -> bool {
    ENGINE_SLOT.try_lock().is_err()
}

#[derive(Default)]
pub(crate) struct LatexBuildHost(Mutex<BuildState>);

#[derive(Default)]
struct BuildState {
    queues: HashMap<String, Queue>,
    runs: HashMap<String, JobRun>,
    /// Projects whose records from earlier sessions were settled.
    settled: HashSet<(String, String)>,
}

struct JobRun {
    project_dir: PathBuf,
    job_dir: PathBuf,
    snapshot: ProjectSnapshot,
    cancel: Arc<AtomicBool>,
    started: bool,
    report: Option<onote_latex::BuildReport>,
}

/// `jobs/<operationId>/job.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobFile {
    format_version: u32,
    job_id: String,
    operation_id: String,
    project_id: String,
    manifest_hash: String,
    state: JobState,
    reason_key: Option<String>,
    artifact_id: Option<String>,
    created_at: u64,
    finished_at: Option<u64>,
}

/// One job as the workspace shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexBuildJob {
    /// The live job id, or the operation id for a job of an earlier session.
    pub job_id: String,
    pub operation_id: String,
    pub state: JobState,
    pub reason_key: Option<String>,
    pub manifest_hash: String,
    pub artifact_id: Option<String>,
    /// From an earlier session; it can no longer be cancelled.
    pub previous_session: bool,
    pub report: Option<onote_latex::BuildReport>,
}

/// The published PDF, without its bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexArtifactView {
    pub artifact_id: String,
    pub job_id: String,
    pub manifest_hash: String,
    pub label: String,
    pub bytes: u64,
    pub sha256: String,
    pub pages: u32,
    pub synctex: bool,
    pub built_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexBuildStatus {
    /// Newest first.
    pub jobs: Vec<LatexBuildJob>,
    pub artifact: Option<LatexArtifactView>,
}

fn artifact_view(record: ArtifactRecord) -> LatexArtifactView {
    LatexArtifactView {
        artifact_id: record.artifact_id,
        job_id: record.job_id,
        manifest_hash: record.manifest_hash,
        label: record.label,
        bytes: record.bytes,
        sha256: record.sha256,
        pages: record.pages,
        synctex: record.synctex,
        built_at: record.built_at,
    }
}

/// A directory name for an id the frontend sent: never the raw text.
fn segment(id: &str) -> String {
    sha256_hex(id.as_bytes())[..32].to_owned()
}

fn project_dir(builds: &Path, vault_id: &str, project_id: &str) -> PathBuf {
    builds.join(segment(vault_id)).join(segment(project_id))
}

fn write_job(job_dir: &Path, file: &JobFile) {
    if let Ok(json) = serde_json::to_vec_pretty(file) {
        let temporary = job_dir.join("job.json.tmp");
        if std::fs::write(&temporary, json).is_ok() {
            let _ = std::fs::rename(&temporary, job_dir.join("job.json"));
        }
    }
}

fn read_job(job_dir: &Path) -> Option<JobFile> {
    serde_json::from_slice(&std::fs::read(job_dir.join("job.json")).ok()?).ok()
}

fn write_report(job_dir: &Path, report: &onote_latex::BuildReport) {
    if let Ok(json) = serde_json::to_vec(report) {
        let _ = std::fs::write(job_dir.join("report.json"), json);
    }
}

fn read_report(job_dir: &Path) -> Option<onote_latex::BuildReport> {
    serde_json::from_slice(&std::fs::read(job_dir.join("report.json")).ok()?).ok()
}

/// Removes everything a job used except its record and report.
fn clear_work(job_dir: &Path) {
    for name in ["input", "output", "check", "scratch"] {
        let _ = std::fs::remove_dir_all(job_dir.join(name));
    }
}

/// Jobs of earlier sessions that never finished become interrupted; only the
/// newest `KEEP_JOBS` records stay.
fn settle_previous(project_dir: &Path, live: &HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(project_dir.join("jobs")) else {
        return;
    };
    let mut records: Vec<(u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(mut file) = read_job(&path) else {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        };
        if live.contains(&file.operation_id) {
            continue;
        }
        if !file.state.is_terminal() {
            file.state = JobState::Interrupted;
            file.reason_key = Some("build.interrupted".to_owned());
            file.artifact_id = None;
            file.finished_at = Some(compile::now_secs());
            write_job(&path, &file);
        }
        clear_work(&path);
        records.push((file.created_at, path));
    }
    records.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, path) in records.into_iter().skip(KEEP_JOBS) {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Engine file names are paths inside the snapshot; show them relative to the
/// project, and leave anything else (a package in the cache) as written.
fn relative_to_snapshot(input: &Path, file: &str) -> String {
    let cleaned = file.trim_start_matches("./").replace('\\', "/");
    let base = input.to_string_lossy().replace('\\', "/");
    let base = base.trim_end_matches('/');
    if cleaned.len() > base.len()
        && cleaned[..base.len()].eq_ignore_ascii_case(base)
        && cleaned.as_bytes()[base.len()] == b'/'
    {
        return cleaned[base.len() + 1..].to_owned();
    }
    cleaned
}

/// A path as the engine wrote it, relative to the project. Paths inside a job's
/// snapshot have their job folder taken off; anything else is not the user's file,
/// so it is never offered as a jump target and never reaches the webview.
fn project_relative(project_dir: &Path, engine_path: &str) -> Option<String> {
    let cleaned = engine_path.replace('\\', "/");
    let base = project_dir.to_string_lossy().replace('\\', "/");
    let jobs = format!("{}/jobs/", base.trim_end_matches('/'));
    let inside = if cleaned.len() > jobs.len() && cleaned[..jobs.len()].eq_ignore_ascii_case(&jobs)
    {
        cleaned[jobs.len()..].split_once("/input/")?.1.to_owned()
    } else if cleaned.as_bytes().get(1) == Some(&b':') || cleaned.starts_with('/') {
        return None;
    } else {
        cleaned
    };
    let parts: Vec<&str> = inside
        .split('/')
        .filter(|part| *part != "." && !part.is_empty())
        .collect();
    (!parts.is_empty() && !parts.contains(&"..")).then(|| parts.join("/"))
}

/// Largest decompressed SyncTeX file this reads.
const MAX_SYNCTEX_BYTES: u64 = 512 * 1024 * 1024;

/// The index of the artifact used last: a click should not re-read the file.
static SYNC_CACHE: Mutex<Option<(String, Arc<onote_latex::SyncIndex>)>> = Mutex::new(None);

/// The mapping of the current artifact. Refused if the source has changed since it
/// was built, because a line number from then may be a different line now (F52-A01).
fn sync_index(
    project_dir: &Path,
    artifact_id: &str,
    expected_manifest_hash: &str,
) -> Result<Arc<onote_latex::SyncIndex>, ServiceError> {
    let record = compile::current_artifact(project_dir)
        .filter(|record| record.artifact_id == artifact_id)
        .ok_or_else(|| svc_err(ErrorCode::VersionConflict, "error.synctexStale"))?;
    if record.manifest_hash != expected_manifest_hash {
        return Err(svc_err(ErrorCode::VersionConflict, "error.synctexStale"));
    }
    if !record.synctex {
        return Err(svc_err(
            ErrorCode::ObjectNotFound,
            "error.synctexUnavailable",
        ));
    }
    if let Ok(cache) = SYNC_CACHE.lock() {
        if let Some((id, index)) = cache.as_ref() {
            if id == artifact_id {
                return Ok(index.clone());
            }
        }
    }
    let unavailable = || svc_err(ErrorCode::ObjectNotFound, "error.synctexUnavailable");
    let file = std::fs::File::open(
        compile::artifact_dir(project_dir, artifact_id).join("document.synctex.gz"),
    )
    .map_err(|_| unavailable())?;
    let mut text = String::new();
    flate2::read::GzDecoder::new(file)
        .take(MAX_SYNCTEX_BYTES)
        .read_to_string(&mut text)
        .map_err(|_| unavailable())?;
    let index = Arc::new(onote_latex::parse_synctex(&text));
    if let Ok(mut cache) = SYNC_CACHE.lock() {
        *cache = Some((artifact_id.to_owned(), index.clone()));
    }
    Ok(index)
}

/// How far from the asked line output still counts, for a comment or a blank line.
const SYNC_REACH_LINES: u32 = 20;

fn forward_in(
    project_dir: &Path,
    index: &onote_latex::SyncIndex,
    path: &str,
    line: u32,
) -> Option<onote_latex::PdfSpot> {
    let written = index
        .inputs()
        .filter(|(_, written)| project_relative(project_dir, written).as_deref() == Some(path))
        .map(|(_, written)| written.to_owned())
        .collect::<Vec<_>>();
    written
        .iter()
        .find_map(|file| index.forward(file, line, SYNC_REACH_LINES))
}

fn inverse_in(
    project_dir: &Path,
    index: &onote_latex::SyncIndex,
    page: u32,
    x: f64,
    y: f64,
) -> Option<onote_latex::SourceSpot> {
    let spot = index.inverse(page, x, y, |written| {
        project_relative(project_dir, written).is_some()
    })?;
    Some(onote_latex::SourceSpot {
        file: project_relative(project_dir, &spot.file)?,
        line: spot.line,
    })
}

/// F52: where a source line appears in the current PDF.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn synctex_forward(
    app: AppHandle,
    vault_id: String,
    project_id: String,
    artifact_id: String,
    expected_manifest_hash: String,
    path: String,
    line: u32,
) -> Result<Option<onote_latex::PdfSpot>, ServiceError> {
    let directory = project_dir(&host_paths(&app)?, &vault_id, &project_id);
    tauri::async_runtime::spawn_blocking(move || {
        let index = sync_index(&directory, &artifact_id, &expected_manifest_hash)?;
        Ok(forward_in(&directory, &index, &path, line))
    })
    .await
    .map_err(map_join_err)?
}

/// F52: the source line behind a point on a page of the current PDF.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn synctex_inverse(
    app: AppHandle,
    vault_id: String,
    project_id: String,
    artifact_id: String,
    expected_manifest_hash: String,
    page: u32,
    x: f64,
    y: f64,
) -> Result<Option<onote_latex::SourceSpot>, ServiceError> {
    let directory = project_dir(&host_paths(&app)?, &vault_id, &project_id);
    tauri::async_runtime::spawn_blocking(move || {
        let index = sync_index(&directory, &artifact_id, &expected_manifest_hash)?;
        Ok(inverse_in(&directory, &index, page, x, y))
    })
    .await
    .map_err(map_join_err)?
}

fn report_for(
    outcome: &compile::EngineOutcome,
    input: &Path,
    job_id: &str,
    manifest_hash: &str,
) -> onote_latex::BuildReport {
    let text = if outcome.log.is_empty() {
        outcome.stderr.clone()
    } else {
        format!("{}\n{}", outcome.log, outcome.stderr)
    };
    let mut report = onote_latex::parse_build_log(&text, outcome.exit_code.map(|code| code as i32));
    for diagnostic in &mut report.diagnostics {
        if let Some(file) = &diagnostic.file {
            // `\\input{chapters/one}` is logged as written, without its extension.
            let relative = relative_to_snapshot(input, file);
            let with_extension = format!("{relative}.tex");
            diagnostic.file = Some(
                if !input.join(&relative).is_file() && input.join(&with_extension).is_file() {
                    with_extension
                } else {
                    relative
                },
            );
        }
    }
    report.truncated |= outcome.truncated;
    report.job_id = Some(job_id.to_owned());
    report.source_hash = Some(manifest_hash.to_owned());
    report
}

/// Paths and identities one job needs, outside any lock.
struct JobPlan {
    builds: PathBuf,
    active: PathBuf,
    validator_source: PathBuf,
    profile: String,
    project_id: String,
    job_id: String,
    project_dir: PathBuf,
    job_dir: PathBuf,
    snapshot: ProjectSnapshot,
}

/// Runs one job to its end: engine, check, publication.
fn execute(
    plan: &JobPlan,
    cancel: &AtomicBool,
) -> (
    Result<ArtifactRecord, BuildError>,
    Option<onote_latex::BuildReport>,
) {
    let input = plan.job_dir.join("input");
    let output = plan.job_dir.join("output");
    if std::fs::create_dir_all(&output).is_err() {
        return (Err(BuildError::Io), None);
    }
    let engine = compile::run_engine(
        &EngineJob {
            profile: &plan.profile,
            active: &plan.active,
            input: &input,
            entry: &plan.snapshot.entry,
            output: &output,
            timeout: compile::COMPILE_TIMEOUT,
        },
        cancel,
    );
    let outcome = match engine {
        Ok(outcome) => outcome,
        Err(error) => return (Err(error), None),
    };
    let report = report_for(&outcome, &input, &plan.job_id, &plan.snapshot.manifest_hash);
    if outcome.exit_code != Some(0) {
        return (Err(BuildError::EngineFailed), Some(report));
    }
    let Some(pdf) = &outcome.pdf else {
        return (Err(BuildError::NoPdf), Some(report));
    };
    let checked = (|| {
        let size = std::fs::metadata(pdf).map_err(|_| BuildError::Io)?.len();
        if size > crate::pdf_validate::MAX_PDF_BYTES {
            return Err(BuildError::TooLarge);
        }
        // The check reads a copy in a directory holding nothing else; the
        // published bytes must hash to what was checked.
        let check = plan.job_dir.join("check");
        let scratch = plan.job_dir.join("scratch");
        for directory in [&check, &scratch] {
            std::fs::create_dir_all(directory).map_err(|_| BuildError::Io)?;
        }
        let candidate = check.join("document.pdf");
        std::fs::copy(pdf, &candidate).map_err(|_| BuildError::Io)?;
        let sha = sha256_hex(&std::fs::read(&candidate).map_err(|_| BuildError::Io)?);
        let validator = compile::validator_copy(&plan.builds, &plan.validator_source)?;
        let pages = compile::validate(
            &validator,
            &plan.profile,
            &check,
            &candidate,
            &scratch,
            cancel,
        )?;
        let entry_name = plan.snapshot.entry.rsplit('/').next().unwrap_or("main.tex");
        let label = format!(
            "{}.pdf",
            entry_name.strip_suffix(".tex").unwrap_or(entry_name)
        );
        compile::publish(
            &plan.project_dir,
            &Publication {
                project_id: &plan.project_id,
                job_id: &plan.job_id,
                manifest_hash: &plan.snapshot.manifest_hash,
                label: &label,
                pdf: &candidate,
                checked_sha256: &sha,
                pages,
                synctex: outcome.synctex.as_deref(),
            },
        )
    })();
    (checked, Some(report))
}

fn job_state_of(error: &BuildError) -> JobState {
    match error {
        BuildError::Cancelled => JobState::Cancelled,
        BuildError::TimedOut => JobState::TimedOut,
        _ => JobState::Failed,
    }
}

fn host_paths(app: &AppHandle) -> Result<PathBuf, ServiceError> {
    app.path()
        .app_local_data_dir()
        .map(|directory| directory.join("latex-builds"))
        .map_err(|_| svc_err(ErrorCode::IoDenied, "error.latexRuntimeUnavailable"))
}

/// Starts a worker for every running job of `project_id` that has none.
fn spawn_ready(app: &AppHandle, state: &mut BuildState, vault_id: &str, project_id: &str) {
    let Some(job_id) = state
        .queues
        .get(vault_id)
        .and_then(|queue| queue.running_job(project_id))
        .map(|job| job.job_id.clone())
    else {
        return;
    };
    let Some(run) = state.runs.get_mut(&job_id) else {
        return;
    };
    if run.started {
        return;
    }
    run.started = true;
    let handle = app.clone();
    let vault = vault_id.to_owned();
    std::thread::spawn(move || run_job(&handle, &vault, &job_id));
}

fn run_job(app: &AppHandle, vault_id: &str, job_id: &str) {
    // Waits for any other project's build to end.
    let slot = ENGINE_SLOT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let host = app.state::<LatexBuildHost>();
    let prepared = (|| {
        let builds = host_paths(app).ok()?;
        let state = host.0.lock().ok()?;
        let run = state.runs.get(job_id)?;
        let job = state.queues.get(vault_id)?.job(job_id)?.clone();
        Some((
            JobPlan {
                builds,
                active: PathBuf::new(),
                validator_source: std::env::current_exe().ok()?,
                profile: onote_latex_sandbox::APP_PROFILE.to_owned(),
                project_id: job.project_id.clone(),
                job_id: job_id.to_owned(),
                project_dir: run.project_dir.clone(),
                job_dir: run.job_dir.clone(),
                snapshot: run.snapshot.clone(),
            },
            run.cancel.clone(),
            job,
        ))
    })();
    let Some((mut plan, cancel, job)) = prepared else {
        return;
    };
    let (result, report) = if cancel.load(Ordering::Relaxed) {
        (Err(BuildError::Cancelled), None)
    } else {
        match crate::latex_runtime::ready_active(app) {
            Ok(active) => {
                plan.active = active;
                execute(&plan, &cancel)
            }
            Err(_) => (Err(BuildError::EnvironmentMissing), None),
        }
    };
    drop(slot);
    clear_work(&plan.job_dir);
    if let Some(report) = &report {
        write_report(&plan.job_dir, report);
    }
    let mut file = JobFile {
        format_version: 1,
        job_id: job_id.to_owned(),
        operation_id: job.operation_id.clone(),
        project_id: job.project_id.clone(),
        manifest_hash: plan.snapshot.manifest_hash.clone(),
        state: JobState::Succeeded,
        reason_key: None,
        artifact_id: None,
        created_at: read_job(&plan.job_dir).map_or(0, |file| file.created_at),
        finished_at: Some(compile::now_secs()),
    };
    let Ok(mut state) = host.0.lock() else {
        return;
    };
    if let Some(run) = state.runs.get_mut(job_id) {
        run.report = report;
    }
    let Some(queue) = state.queues.get_mut(vault_id) else {
        return;
    };
    match &result {
        Ok(artifact) => {
            let manifest = &plan.snapshot.manifest_hash;
            let _ = queue.finish_success(job_id, &artifact.artifact_id, manifest, manifest);
            file.artifact_id = Some(artifact.artifact_id.clone());
        }
        Err(error) => {
            let state_value = job_state_of(error);
            let _ = queue.finish(job_id, state_value, Some(error.reason_key()));
            queue.start_pending(&job.project_id);
            file.state = state_value;
            file.reason_key = Some(error.reason_key().to_owned());
        }
    }
    write_job(&plan.job_dir, &file);
    spawn_ready(app, &mut state, vault_id, &job.project_id);
}

/// F51: requests a build of the project as it is on disk now.
#[tauri::command]
pub(crate) fn compile_latex_project(
    app: AppHandle,
    vault_id: String,
    project_id: String,
    expected_manifest_hash: String,
    operation_id: String,
) -> Result<onote_latex::CompileOutcome, ServiceError> {
    if uuid::Uuid::parse_str(&operation_id).is_err() {
        return Err(svc_err(ErrorCode::InvalidInput, "error.invalidOperationId"));
    }
    let builds = host_paths(&app)?;
    let host = app.state::<LatexBuildHost>();
    let replay = |state: &mut BuildState| {
        let queue = state.queues.entry(vault_id.clone()).or_default();
        queue
            .jobs()
            .iter()
            .find(|job| job.operation_id == operation_id)
            .map(|job| {
                if job.project_id != project_id || job.input.manifest_hash != expected_manifest_hash
                {
                    Err(svc_err(ErrorCode::VersionConflict, "error.operationReused"))
                } else {
                    Ok(onote_latex::CompileOutcome {
                        job_id: job.job_id.clone(),
                        state: job.state,
                        created: false,
                    })
                }
            })
    };
    {
        let mut state = host.0.lock().map_err(|_| poison_err())?;
        if let Some(outcome) = replay(&mut state) {
            return outcome;
        }
    }
    // Nothing is snapshotted for an environment that cannot build it.
    crate::latex_runtime::ready_active(&app)?;
    let project_dir = project_dir(&builds, &vault_id, &project_id);
    let job_dir = project_dir.join("jobs").join(&operation_id);
    std::fs::create_dir_all(&job_dir)
        .map_err(|_| svc_err(ErrorCode::IoDenied, "error.buildSnapshotFailed"))?;
    let captured = with_state(&app, |state| {
        capture_snapshot(
            scoped_vault_mut(state, &vault_id)?,
            &project_id,
            &expected_manifest_hash,
            &job_dir.join("input"),
        )
    });
    let snapshot = match captured {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&job_dir);
            return Err(error);
        }
    };
    let mut state = host.0.lock().map_err(|_| poison_err())?;
    if let Some(outcome) = replay(&mut state) {
        let _ = std::fs::remove_dir_all(&job_dir);
        return outcome;
    }
    let queue = state.queues.entry(vault_id.clone()).or_default();
    let waiting = queue.queued_job(&project_id).map(|job| job.job_id.clone());
    let input = onote_latex::JobInput {
        manifest_hash: snapshot.manifest_hash.clone(),
        source_hash: snapshot.manifest_hash.clone(),
    };
    let outcome = queue
        .request(
            &project_id,
            &expected_manifest_hash,
            &snapshot.manifest_hash,
            input,
            &operation_id,
        )
        .map_err(|_| svc_err(ErrorCode::VersionConflict, "error.buildManifestStale"))?;
    // A waiting job replaced by this request ends here, with its snapshot.
    if let Some(previous) = waiting.filter(|id| *id != outcome.job_id) {
        if let Some(run) = state.runs.remove(&previous) {
            let _ = std::fs::remove_dir_all(&run.job_dir);
        }
    }
    if !outcome.created {
        let _ = std::fs::remove_dir_all(&job_dir);
        return Ok(outcome);
    }
    write_job(
        &job_dir,
        &JobFile {
            format_version: 1,
            job_id: outcome.job_id.clone(),
            operation_id: operation_id.clone(),
            project_id: project_id.clone(),
            manifest_hash: snapshot.manifest_hash.clone(),
            state: outcome.state,
            reason_key: None,
            artifact_id: None,
            created_at: compile::now_secs(),
            finished_at: None,
        },
    );
    state.runs.insert(
        outcome.job_id.clone(),
        JobRun {
            project_dir,
            job_dir,
            snapshot,
            cancel: Arc::new(AtomicBool::new(false)),
            started: false,
            report: None,
        },
    );
    spawn_ready(&app, &mut state, &vault_id, &project_id);
    Ok(outcome)
}

/// F51: cancels a job. A running one is stopped with its whole process tree;
/// a waiting one ends without running. The previous PDF stays.
#[tauri::command]
pub(crate) fn cancel_latex_compile(
    app: AppHandle,
    vault_id: String,
    job_id: String,
) -> Result<JobState, ServiceError> {
    let host = app.state::<LatexBuildHost>();
    let mut state = host.0.lock().map_err(|_| poison_err())?;
    let queue = state
        .queues
        .get_mut(&vault_id)
        .ok_or_else(|| svc_err(ErrorCode::ObjectNotFound, "error.buildJobMissing"))?;
    let job = queue
        .job(&job_id)
        .cloned()
        .ok_or_else(|| svc_err(ErrorCode::ObjectNotFound, "error.buildJobMissing"))?;
    match job.state {
        JobState::Running | JobState::Preparing => {
            if let Some(run) = state.runs.get(&job_id) {
                run.cancel.store(true, Ordering::Relaxed);
            }
            Ok(job.state)
        }
        JobState::Queued => {
            queue
                .finish(&job_id, JobState::Cancelled, Some("build.cancelled"))
                .map_err(|_| svc_err(ErrorCode::VersionConflict, "error.buildJobFinished"))?;
            if let Some(run) = state.runs.remove(&job_id) {
                let _ = std::fs::remove_dir_all(&run.job_dir);
            }
            Ok(JobState::Cancelled)
        }
        finished => Ok(finished),
    }
}

/// F51/F52: the project's jobs (this session's and recorded earlier ones) and
/// its current published PDF, re-verified against its hash.
#[tauri::command]
pub(crate) async fn latex_build_status(
    app: AppHandle,
    vault_id: String,
    project_id: String,
) -> Result<LatexBuildStatus, ServiceError> {
    let builds = host_paths(&app)?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let project_dir = project_dir(&builds, &vault_id, &project_id);
        let host = handle.state::<LatexBuildHost>();
        let mut jobs = Vec::new();
        let mut live = HashSet::new();
        {
            let mut state = host.0.lock().map_err(|_| poison_err())?;
            if let Some(queue) = state.queues.get(&vault_id) {
                for job in queue.jobs().iter().rev() {
                    if job.project_id != project_id {
                        continue;
                    }
                    live.insert(job.operation_id.clone());
                    jobs.push(LatexBuildJob {
                        job_id: job.job_id.clone(),
                        operation_id: job.operation_id.clone(),
                        state: job.state,
                        reason_key: job.reason_key.clone(),
                        manifest_hash: job.input.manifest_hash.clone(),
                        artifact_id: job.artifact_id.clone(),
                        previous_session: false,
                        report: state
                            .runs
                            .get(&job.job_id)
                            .and_then(|run| run.report.clone()),
                    });
                }
            }
            if state.settled.insert((vault_id.clone(), project_id.clone())) {
                settle_previous(&project_dir, &live);
            }
        }
        let mut earlier: Vec<(u64, LatexBuildJob)> = std::fs::read_dir(project_dir.join("jobs"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let file = read_job(&entry.path())?;
                (!live.contains(&file.operation_id) && file.state.is_terminal()).then(|| {
                    (
                        file.created_at,
                        LatexBuildJob {
                            job_id: file.operation_id.clone(),
                            operation_id: file.operation_id,
                            state: file.state,
                            reason_key: file.reason_key,
                            manifest_hash: file.manifest_hash,
                            artifact_id: file.artifact_id,
                            previous_session: true,
                            report: read_report(&entry.path()),
                        },
                    )
                })
            })
            .collect();
        earlier.sort_by(|left, right| right.0.cmp(&left.0));
        jobs.extend(earlier.into_iter().map(|(_, job)| job));
        Ok(LatexBuildStatus {
            jobs,
            artifact: compile::current_artifact(&project_dir).map(artifact_view),
        })
    })
    .await
    .map_err(map_join_err)?
}

/// F52: one bounded chunk of the current PDF, as raw bytes. The frontend
/// never receives a path; it checks the assembled bytes against `sha256`.
#[tauri::command]
pub(crate) async fn read_latex_artifact(
    app: AppHandle,
    vault_id: String,
    project_id: String,
    artifact_id: String,
    offset: u64,
    length: u64,
) -> Result<tauri::ipc::Response, ServiceError> {
    if uuid::Uuid::parse_str(&artifact_id).is_err() {
        return Err(svc_err(
            ErrorCode::InvalidInput,
            "error.buildArtifactMissing",
        ));
    }
    let builds = host_paths(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        compile::read_chunk(
            &project_dir(&builds, &vault_id, &project_id),
            &artifact_id,
            offset,
            length,
        )
        .map(tauri::ipc::Response::new)
        .map_err(|_| svc_err(ErrorCode::ObjectNotFound, "error.buildArtifactMissing"))
    })
    .await
    .map_err(map_join_err)?
}

/// The current published PDF's bytes and file name, checked against the hash
/// recorded when it was validated.
fn current_pdf(project_dir: &Path) -> Result<(String, Vec<u8>), ServiceError> {
    let missing = || svc_err(ErrorCode::ObjectNotFound, "error.buildArtifactMissing");
    let record = compile::current_artifact(project_dir).ok_or_else(missing)?;
    let bytes =
        std::fs::read(compile::artifact_dir(project_dir, &record.artifact_id).join("document.pdf"))
            .map_err(|_| missing())?;
    if sha256_hex(&bytes) != record.sha256 {
        return Err(missing());
    }
    Ok((record.label, bytes))
}

/// Saves a copy of the current PDF where the user picks.
#[tauri::command]
pub(crate) async fn save_latex_pdf(
    app: AppHandle,
    vault_id: String,
    project_id: String,
    dialog_title: Option<String>,
) -> Result<bool, ServiceError> {
    let builds = host_paths(&app)?;
    let title = localized_or_fallback(dialog_title, "Save PDF");
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (name, bytes) = current_pdf(&project_dir(&builds, &vault_id, &project_id))?;
        save_with_dialog(&handle, &bytes, &name, &title, ("PDF", &["pdf"]))
    })
    .await
    .map_err(map_join_err)?
}

/// Every job of the vault, for the queue panel.
#[tauri::command]
pub(crate) fn list_compile_jobs(
    app: AppHandle,
    vault_id: String,
) -> Result<Vec<onote_latex::Job>, ServiceError> {
    let host = app.state::<LatexBuildHost>();
    let state = host.0.lock().map_err(|_| poison_err())?;
    Ok(state
        .queues
        .get(&vault_id)
        .map(|queue| queue.jobs().to_vec())
        .unwrap_or_default())
}

/// Running and waiting jobs in the vault, for the diagnostics probe.
pub(crate) fn queue_counts(app: &AppHandle, vault_id: &str) -> Option<(u64, u64)> {
    let host = app.state::<LatexBuildHost>();
    let state = host.0.lock().ok()?;
    let jobs = state
        .queues
        .get(vault_id)
        .map(|queue| queue.jobs().to_vec())
        .unwrap_or_default();
    let count = |wanted: JobState| jobs.iter().filter(|job| job.state == wanted).count() as u64;
    Some((count(JobState::Running), count(JobState::Queued)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_saved_pdf_is_the_current_checked_one_and_a_tampered_copy_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        assert_eq!(
            current_pdf(&project).unwrap_err().code,
            ErrorCode::ObjectNotFound
        );
        let candidate = directory.path().join("built.pdf");
        std::fs::write(&candidate, b"%PDF-1.7 body").unwrap();
        let record = compile::publish(
            &project,
            &Publication {
                project_id: "p",
                job_id: "j",
                manifest_hash: "m",
                label: "main.pdf",
                pdf: &candidate,
                checked_sha256: &sha256_hex(b"%PDF-1.7 body"),
                pages: 1,
                synctex: None,
            },
        )
        .unwrap();
        assert_eq!(
            current_pdf(&project).unwrap(),
            ("main.pdf".to_owned(), b"%PDF-1.7 body".to_vec())
        );
        std::fs::write(
            compile::artifact_dir(&project, &record.artifact_id).join("document.pdf"),
            b"changed",
        )
        .unwrap();
        assert_eq!(
            current_pdf(&project).unwrap_err().code,
            ErrorCode::ObjectNotFound
        );
    }

    #[test]
    fn only_paths_in_a_job_snapshot_count_as_project_files() {
        let project = Path::new(r"C:\Data\builds\a\b");
        let relative = |path: &str| project_relative(project, path);
        assert_eq!(
            relative(r"C:\Data\builds\a\b\jobs\op\input\main.tex").as_deref(),
            Some("main.tex")
        );
        assert_eq!(
            relative(r"C:\Data\builds\a\b\jobs\op\input\./ch/one.tex").as_deref(),
            Some("ch/one.tex")
        );
        assert_eq!(
            relative("c:/data/BUILDS/a/b/jobs/op/input/main.tex").as_deref(),
            Some("main.tex")
        );
        assert_eq!(relative("./ch/one.tex").as_deref(), Some("ch/one.tex"));
        assert_eq!(relative(r"C:\runtime\cache\article.cls"), None);
        assert_eq!(relative(r"C:\Data\builds\a\b\jobs\op\other\main.tex"), None);
        assert_eq!(relative("../outside.tex"), None);
        assert_eq!(relative(""), None);
    }

    #[test]
    fn engine_paths_inside_the_snapshot_become_project_paths() {
        let input = Path::new(r"C:\Data\latex-builds\a\b\jobs\op\input");
        assert_eq!(
            relative_to_snapshot(
                input,
                r"C:\Data\latex-builds\a\b\jobs\op\input\chapters\one.tex"
            ),
            "chapters/one.tex"
        );
        assert_eq!(
            relative_to_snapshot(input, "c:/data/LATEX-BUILDS/a/b/jobs/op/input/main.tex"),
            "main.tex"
        );
        assert_eq!(
            relative_to_snapshot(input, "./chapters/one.tex"),
            "chapters/one.tex"
        );
        // A file elsewhere keeps its name and is never mapped into the project.
        assert_eq!(
            relative_to_snapshot(input, r"C:\Data\latex-builds\a\b\jobs\op\inputx\main.tex"),
            "C:/Data/latex-builds/a/b/jobs/op/inputx/main.tex"
        );
    }

    #[test]
    fn unfinished_jobs_of_an_earlier_session_become_interrupted_and_old_ones_go() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("p");
        for index in 0..(KEEP_JOBS + 2) {
            let job_dir = project.join("jobs").join(format!("op{index}"));
            std::fs::create_dir_all(job_dir.join("input")).unwrap();
            write_job(
                &job_dir,
                &JobFile {
                    format_version: 1,
                    job_id: format!("job-{index}"),
                    operation_id: format!("op{index}"),
                    project_id: "p".to_owned(),
                    manifest_hash: "m".to_owned(),
                    state: if index == KEEP_JOBS + 1 {
                        JobState::Running
                    } else {
                        JobState::Failed
                    },
                    reason_key: None,
                    artifact_id: Some("a".to_owned()),
                    created_at: index as u64,
                    finished_at: None,
                },
            );
        }
        settle_previous(&project, &HashSet::new());
        let newest = read_job(&project.join("jobs").join(format!("op{}", KEEP_JOBS + 1))).unwrap();
        assert_eq!(newest.state, JobState::Interrupted);
        assert_eq!(newest.artifact_id, None);
        assert!(!project
            .join("jobs")
            .join(format!("op{}", KEEP_JOBS + 1))
            .join("input")
            .exists());
        let kept = std::fs::read_dir(project.join("jobs")).unwrap().count();
        assert_eq!(kept, KEEP_JOBS);
        assert!(!project.join("jobs/op0").exists());
    }

    /// A prepared environment for the engine tests: `ONOTE_LATEX_FIXTURE_ROOT`
    /// if it verifies, otherwise prepared there once from
    /// `ONOTE_TECTONIC_ARCHIVE` (needs internet for the resources).
    fn fixture_active() -> PathBuf {
        use crate::latex_runtime::environment;
        let root = PathBuf::from(
            std::env::var_os("ONOTE_LATEX_FIXTURE_ROOT").expect("set ONOTE_LATEX_FIXTURE_ROOT"),
        );
        if !matches!(
            environment::verify(&root),
            environment::EnvironmentState::Ready { .. }
        ) {
            let id = std::process::id();
            let profiles = [
                format!("opennote.sandboxtest.fixture.prep.{id}"),
                format!("opennote.sandboxtest.fixture.comp.{id}"),
            ];
            let fixed = environment::fixed_manifest();
            let (deadline, download_deadline) = environment::deadlines(&fixed);
            let request = environment::PrepareRequest {
                operation_id: uuid::Uuid::new_v4().to_string(),
                engine: environment::EngineSource::File(PathBuf::from(
                    std::env::var_os("ONOTE_TECTONIC_ARCHIVE").expect("set ONOTE_TECTONIC_ARCHIVE"),
                )),
                templates: crate::latex_runtime::preparation_templates(),
                prepare_profile: profiles[0].clone(),
                compile_profile: profiles[1].clone(),
                deadline,
                download_deadline,
                max_resource_bytes: fixed.resources.max_bytes,
            };
            let progress = Mutex::new(environment::Progress {
                stage: environment::Stage::Downloading,
                bytes: 0,
                total_bytes: None,
                step: 0,
                steps: 0,
            });
            let outcome = environment::prepare(&root, &request, &AtomicBool::new(false), &progress);
            let refs: Vec<&str> = profiles.iter().map(String::as_str).collect();
            crate::latex_runtime::remove_everything(None, &refs);
            outcome.unwrap_or_else(|error| panic!("fixture preparation failed: {error:?}"));
        }
        environment::active_dir(&root)
    }

    struct Fixture {
        _base: tempfile::TempDir,
        profile: String,
        plan: JobPlan,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let errors =
                crate::latex_runtime::remove_everything(None, &[self.profile.as_str()]).errors;
            if !errors.is_empty() {
                eprintln!("CLEANUP ERRORS {errors:?}");
            }
        }
    }

    /// A registered vault project with `files`, snapshotted for one job.
    fn fixture(tag: &str, files: &[(&str, &str)]) -> Fixture {
        let binary: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(path, text)| (*path, text.as_bytes()))
            .collect();
        fixture_binary(tag, &binary)
    }

    /// The same, for a project that also holds figures.
    fn fixture_binary(tag: &str, files: &[(&str, &[u8])]) -> Fixture {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("vault");
        for (path, text) in files {
            let target = root.join("paper").join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, text).unwrap();
        }
        let mut storage = VaultStorage::create(&root, "Build").unwrap();
        let project = crate::latex_project::register_folder(
            &mut storage,
            "paper",
            "main.tex",
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        let builds = base.path().join("latex-builds");
        let project_dir = project_dir(&builds, "vault", &project.project_id);
        let job_dir = project_dir
            .join("jobs")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&job_dir).unwrap();
        let snapshot = capture_snapshot(
            &mut storage,
            &project.project_id,
            &project.manifest_hash,
            &job_dir.join("input"),
        )
        .unwrap();
        // The test harness is not the program; the built binary is beside it.
        let program = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("src-tauri.exe");
        assert!(program.is_file(), "build the src-tauri binary first");
        let profile = format!("opennote.sandboxtest.build.{tag}.{}", std::process::id());
        Fixture {
            plan: JobPlan {
                builds,
                active: fixture_active(),
                validator_source: program,
                profile: profile.clone(),
                project_id: project.project_id,
                job_id: "job-1".to_owned(),
                project_dir,
                job_dir,
                snapshot,
            },
            profile,
            _base: base,
        }
    }

    /// The preamble of the built-in templates: what a prepared environment
    /// is guaranteed to cover offline.
    const PREAMBLE: &str = "\\documentclass[11pt,a4paper]{article}\n\\usepackage[T1]{fontenc}\n\\usepackage[utf8]{inputenc}\n";

    fn main_with(body: &str) -> String {
        format!("{PREAMBLE}\\begin{{document}}\n{body}\\end{{document}}\n")
    }

    #[test]
    #[ignore = "needs a prepared environment (ONOTE_LATEX_FIXTURE_ROOT)"]
    fn every_template_builds_offline_from_a_registered_project_into_a_checked_pdf() {
        for (name, files) in crate::latex_runtime::preparation_templates() {
            let files: Vec<(&str, &str)> = files
                .iter()
                .map(|(path, text)| (path.as_str(), text.as_str()))
                .collect();
            let fixture = fixture(&name, &files);
            let started = std::time::Instant::now();
            let (result, report) = execute(&fixture.plan, &AtomicBool::new(false));
            let report = report.expect("a report");
            let artifact = result
                .unwrap_or_else(|error| panic!("{name}: {error:?}: {:?}", report.diagnostics));
            eprintln!(
                "BUILD {name} seconds={} bytes={} pages={}",
                started.elapsed().as_secs(),
                artifact.bytes,
                artifact.pages
            );
            assert!(artifact.pages >= 1);
            assert_eq!(artifact.label, "main.pdf");
            assert_eq!(artifact.manifest_hash, fixture.plan.snapshot.manifest_hash);
            assert!(artifact.synctex);
            assert!(report.succeeded);
            assert_eq!(
                report.source_hash.as_deref(),
                Some(artifact.manifest_hash.as_str())
            );
            assert_eq!(
                compile::current_artifact(&fixture.plan.project_dir),
                Some(artifact.clone())
            );
            let head = compile::read_chunk(&fixture.plan.project_dir, &artifact.artifact_id, 0, 5)
                .unwrap();
            assert_eq!(head, b"%PDF-");
            // Nothing of the job is left but its record.
            clear_work(&fixture.plan.job_dir);
        }
    }
    #[test]
    #[ignore = "needs a prepared environment (ONOTE_LATEX_FIXTURE_ROOT)"]
    fn a_real_build_maps_lines_to_the_page_and_back_across_files() {
        let fixture = fixture(
            "sync",
            &[
                (
                    "main.tex",
                    &main_with("First paragraph here.\n\n\\newpage\n\n\\input{chapters/one}\n"),
                ),
                (
                    "chapters/one.tex",
                    "Comment below.\n% nothing\n\nSecond chapter text on the later page.\n",
                ),
            ],
        );
        let (result, _) = execute(&fixture.plan, &AtomicBool::new(false));
        let artifact = result.expect("built");
        let project = &fixture.plan.project_dir;
        let index =
            sync_index(project, &artifact.artifact_id, &artifact.manifest_hash).expect("a mapping");
        let inputs: Vec<String> = index
            .inputs()
            .filter_map(|(_, path)| project_relative(project, path))
            .collect();
        eprintln!("INPUTS {inputs:?}");
        assert!(
            inputs.iter().any(|path| path == "chapters/one.tex"),
            "{inputs:?}"
        );
        assert!(
            inputs.iter().all(|path| !path.contains(':')),
            "no absolute path escapes: {inputs:?}"
        );

        let later = forward_in(project, &index, "chapters/one.tex", 4)
            .expect("the chapter line is on a page");
        eprintln!("SPOT {later:?}");
        assert_eq!(later.page, 2, "after the page break");
        assert!(
            later.x > 0.0 && later.y > 0.0 && later.y < 842.0,
            "on the page: {later:?}"
        );

        assert!(
            later.height > 0.0 && later.width > 0.0,
            "marks the printed line: {later:?}"
        );

        let back = inverse_in(
            project,
            &index,
            later.page,
            later.x + 5.0,
            later.y + later.height / 2.0,
        )
        .expect("a line");
        assert_eq!(back.file, "chapters/one.tex");
        assert_eq!(back.line, 4);
        assert_eq!(forward_in(project, &index, "no/such.tex", 1), None);

        // A source that moved on no longer has a precise answer (F52-A01).
        assert_eq!(
            sync_index(project, &artifact.artifact_id, "another-hash")
                .err()
                .map(|error| error.message_key),
            Some("error.synctexStale".to_owned())
        );
    }

    #[test]
    #[ignore = "needs a prepared environment (ONOTE_LATEX_FIXTURE_ROOT)"]
    fn a_project_with_real_images_builds() {
        // A 1x1 PNG and JPEG, and a one-page PDF, as a user's figures would be.
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52, 0,
            0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0x90, 0x77, 0x53, 0xDE, 0, 0, 0, 12, 0x49, 0x44,
            0x41, 0x54, 8, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0, 0, 3, 1, 1, 0, 0x18, 0xDD, 0x8D, 0xB0,
            0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let body = "\\includegraphics[width=2cm]{figures/dot.png}\n";
        let main = main_with(body).replacen(
            "\\begin{document}",
            "\\usepackage{graphicx}\n\\begin{document}",
            1,
        );
        let fixture = fixture_binary(
            "image",
            &[("main.tex", main.as_bytes()), ("figures/dot.png", PNG)],
        );
        let (result, report) = execute(&fixture.plan, &AtomicBool::new(false));
        if let Some(report) = &report {
            eprintln!(
                "MISSING {:?}\nDIAGNOSTICS {:#?}",
                report.missing, report.diagnostics
            );
        }
        let artifact = result.expect("a document with a figure builds offline");
        assert!(artifact.bytes > 0);
    }

    #[test]
    #[ignore = "needs a prepared environment (ONOTE_LATEX_FIXTURE_ROOT)"]
    fn a_failing_project_reports_located_diagnostics_and_publishes_nothing() {
        let fixture = fixture(
            "fail",
            &[
                ("main.tex", &main_with("\\input{chapters/one}\n")),
                ("chapters/one.tex", "Fine.\n\n\\undefinedmacro\n"),
            ],
        );
        let (result, report) = execute(&fixture.plan, &AtomicBool::new(false));
        assert_eq!(result.unwrap_err(), BuildError::EngineFailed);
        let report = report.expect("a report");
        eprintln!("DIAGNOSTICS {:#?}", report.diagnostics);
        assert!(!report.succeeded);
        let error = report
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code == onote_latex::DiagnosticCode::UndefinedControlSequence
            })
            .expect("the undefined macro is reported");
        assert_eq!(error.line, Some(3));
        assert_eq!(
            error.file.as_deref(),
            Some("chapters/one.tex"),
            "the log says which file was open"
        );
        assert_eq!(compile::current_artifact(&fixture.plan.project_dir), None);
    }

    #[test]
    #[ignore = "needs a prepared environment (ONOTE_LATEX_FIXTURE_ROOT)"]
    fn a_project_cannot_read_outside_its_snapshot_or_run_commands() {
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("secret.tex");
        std::fs::write(&sentinel, "SECRET-CONTENT\n").unwrap();
        let path = sentinel.to_string_lossy().replace('\\', "/");
        let main = format!(
            "{PREAMBLE}\\begin{{document}}\n\\immediate\\write18{{echo pwned > pwned.txt}}\n\\input{{{path}}}\n\\end{{document}}\n"
        );
        let fixture = fixture("escape", &[("main.tex", main.as_str())]);
        let (result, _report) = execute(&fixture.plan, &AtomicBool::new(false));
        assert!(result.is_err(), "reading {path} must fail");
        assert!(!fixture
            .plan
            .job_dir
            .join("output")
            .join("pwned.txt")
            .exists());
        assert_eq!(compile::current_artifact(&fixture.plan.project_dir), None);
    }

    #[test]
    #[ignore = "needs a prepared environment (ONOTE_LATEX_FIXTURE_ROOT)"]
    fn cancelling_a_runaway_build_stops_it_and_keeps_the_previous_pdf() {
        let main = main_with("Text.\n");
        let good = fixture("cancel", &[("main.tex", main.as_str())]);
        let (first, _) = execute(&good.plan, &AtomicBool::new(false));
        let kept = first.unwrap();
        // Same project, a new snapshot that never finishes.
        let loop_dir = good.plan.project_dir.join("jobs").join("loop");
        std::fs::create_dir_all(loop_dir.join("input")).unwrap();
        std::fs::write(
            loop_dir.join("input").join("main.tex"),
            main_with("\\def\\x{\\x}\\x\n"),
        )
        .unwrap();
        let plan = JobPlan {
            job_dir: loop_dir,
            job_id: "job-2".to_owned(),
            builds: good.plan.builds.clone(),
            active: good.plan.active.clone(),
            validator_source: good.plan.validator_source.clone(),
            profile: good.plan.profile.clone(),
            project_id: good.plan.project_id.clone(),
            project_dir: good.plan.project_dir.clone(),
            snapshot: good.plan.snapshot.clone(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            trigger.store(true, Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        let (result, _) = execute(&plan, &cancel);
        assert_eq!(result.unwrap_err(), BuildError::Cancelled);
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
        assert_eq!(compile::current_artifact(&plan.project_dir), Some(kept));
    }

    #[test]
    fn directory_names_never_carry_caller_text() {
        assert_eq!(segment("../../x").len(), 32);
        assert!(segment("a/b").chars().all(|c| c.is_ascii_hexdigit()));
    }
}
