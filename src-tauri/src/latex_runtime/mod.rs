//! F45/F53 local LaTeX runtime in the host: the preparation the user agrees
//! to, the environment's status, and its removal (also run by the uninstaller
//! through `--latex-uninstall-cleanup`).
//!
//! Network is used only by `prepare_latex_environment`, only after the user
//! saw `latex_environment_plan` and agreed (`consent`), and only through the
//! fixed engine URL plus the engine itself in the internet-only preparation
//! profile. Compiles use the no-network profile and a verified, read-only
//! environment.

pub(crate) mod archive;
pub(crate) mod compile;
mod download;
pub(crate) mod environment;
mod samples;

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

use environment::{EnvironmentState, Progress, Stage};
use onote_latex_sandbox::{APP_PROFILE, PREPARE_PROFILE, PRODUCT_PROFILES};

#[derive(Default)]
pub(crate) struct LatexRuntimeHost(Mutex<RuntimeState>);

#[derive(Default)]
struct RuntimeState {
    job: Option<Job>,
    last: Option<PrepareOutcome>,
    /// Verified once per session (and again after any change).
    verified: Option<EnvironmentState>,
}

struct Job {
    operation_id: String,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Progress>>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareOutcome {
    pub operation_id: String,
    pub succeeded: bool,
    pub reason_key: Option<&'static str>,
}

/// What a preparation would download, shown before the user agrees.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexEnvironmentPlan {
    pub engine_name: String,
    pub engine_version: String,
    pub engine_license: String,
    pub engine_host: String,
    pub engine_download_bytes: u64,
    pub resource_host: String,
    pub resource_estimated_bytes: u64,
    pub resource_max_bytes: u64,
    pub prepare_seconds: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatexEnvironmentStatus {
    pub environment: EnvironmentState,
    pub preparing: Option<PreparingView>,
    pub last: Option<PrepareOutcome>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparingView {
    pub operation_id: String,
    pub progress: Progress,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub revoked: usize,
    pub profiles_deleted: usize,
    pub runtime_removed: bool,
    /// Steps that failed; the rest still ran.
    pub errors: Vec<String>,
}

fn host_of(url: &str) -> String {
    url.trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn plan() -> LatexEnvironmentPlan {
    let fixed = environment::fixed_manifest();
    LatexEnvironmentPlan {
        engine_host: host_of(&fixed.engine.url),
        engine_name: fixed.engine.name,
        engine_version: fixed.engine.version,
        engine_license: fixed.engine.license,
        engine_download_bytes: fixed.engine.archive_bytes,
        resource_host: host_of(&fixed.resources.source),
        resource_estimated_bytes: fixed.resources.estimated_bytes,
        resource_max_bytes: fixed.resources.max_bytes,
        prepare_seconds: fixed.limits.prepare_seconds,
    }
}

/// Every built-in template in both document languages, then the common-setup
/// samples (`samples.rs`): the resources these need are what a preparation
/// fetches.
pub(crate) fn preparation_templates() -> Vec<(String, Vec<(String, String)>)> {
    let mut templates = Vec::new();
    for template in onote_latex::scaffold_templates() {
        for language in [
            onote_latex::DocumentLanguage::En,
            onote_latex::DocumentLanguage::ZhCn,
        ] {
            let metadata = onote_latex::Metadata {
                title: "OpenNote".to_owned(),
                author: String::new(),
                date: "2026-01-01".to_owned(),
                language,
                paper: onote_latex::PaperSize::A4,
            };
            let plan = onote_latex::plan_scaffold(template, &metadata, "", &[]);
            let name = format!("{template:?}-{language:?}").to_ascii_lowercase();
            templates.push((
                name,
                plan.files
                    .into_iter()
                    .map(|file| (file.path, file.content))
                    .collect(),
            ));
        }
    }
    templates.extend(samples::preparation_samples());
    templates
}

fn runtime_root(app: &AppHandle) -> Result<PathBuf, ServiceError> {
    app.path()
        .app_local_data_dir()
        .map(|directory| directory.join("latex-runtime"))
        .map_err(|_| svc_err(ErrorCode::IoDenied, "error.latexRuntimeUnavailable"))
}

/// The verified environment's `active/` for a build, or why there is none.
/// Verified once per session (like the status), and never while a
/// preparation is replacing it.
pub(crate) fn ready_active(app: &AppHandle) -> Result<PathBuf, ServiceError> {
    let root = runtime_root(app)?;
    let not_ready = || svc_err(ErrorCode::ObjectNotFound, "error.latexEnvironmentNotReady");
    let host = app.state::<LatexRuntimeHost>();
    {
        let state = host.0.lock().map_err(|_| poison_err())?;
        if state.job.is_some() {
            return Err(svc_err(ErrorCode::FileLocked, "error.latexPrepareRunning"));
        }
        if let Some(verified) = &state.verified {
            return match verified {
                EnvironmentState::Ready { .. } => Ok(environment::active_dir(&root)),
                _ => Err(not_ready()),
            };
        }
    }
    environment::clean_leftovers(&root);
    let verified = environment::verify(&root);
    let ready = matches!(verified, EnvironmentState::Ready { .. });
    host.0.lock().map_err(|_| poison_err())?.verified = Some(verified);
    if ready {
        Ok(environment::active_dir(&root))
    } else {
        Err(not_ready())
    }
}

/// Built PDFs, job records and the validator copy: app-created, outside the
/// vault, removed by the uninstaller.
fn uninstall_builds_root() -> Option<PathBuf> {
    uninstall_root()?
        .parent()
        .map(|parent| parent.join("latex-builds"))
}

/// Revokes the approved profile-folder entries of `profiles`, deletes the
/// profiles, and removes `root`. Every step runs even if an earlier one failed.
pub(crate) fn remove_everything(root: Option<&Path>, profiles: &[&str]) -> CleanupReport {
    let mut report = CleanupReport::default();
    match onote_latex_sandbox::profile_folders() {
        Ok(folders) => {
            for profile in profiles {
                for folder in &folders {
                    match onote_latex_sandbox::revoke(profile, folder) {
                        Ok(()) => report.revoked += 1,
                        Err(error) => report.errors.push(format!("revoke {profile}: {error}")),
                    }
                }
            }
        }
        Err(error) => report.errors.push(format!("profileFolders: {error}")),
    }
    for profile in profiles {
        match onote_latex_sandbox::delete_profile(profile) {
            Ok(()) => report.profiles_deleted += 1,
            // HRESULT_FROM_WIN32(ERROR_NOT_FOUND / ERROR_FILE_NOT_FOUND): already gone.
            Err(error) if matches!(error.code, 0x8007_0490 | 0x8007_0002) => {}
            Err(error) => report
                .errors
                .push(format!("deleteProfile {profile}: {error}")),
        }
    }
    if let Some(root) = root {
        match std::fs::remove_dir_all(root) {
            Ok(()) => report.runtime_removed = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => report.errors.push(format!("removeRuntime: {error}")),
        }
    }
    report
}

/// The formal app's runtime directory, for the uninstaller (no app handle).
fn uninstall_root() -> Option<PathBuf> {
    let identifier: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.conf.json")).ok()?;
    let identifier = identifier.get("identifier")?.as_str()?.to_owned();
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join(identifier).join("latex-runtime"))
}

/// `OpenNote.exe --latex-uninstall-cleanup`: run by the uninstaller before
/// files are removed. Returns the process exit code (0 when nothing failed).
pub fn uninstall_cleanup() -> i32 {
    let report = remove_everything(uninstall_root().as_deref(), &PRODUCT_PROFILES);
    let builds_removed = match uninstall_builds_root().map(std::fs::remove_dir_all) {
        None | Some(Ok(())) => true,
        Some(Err(error)) => error.kind() == std::io::ErrorKind::NotFound,
    };
    if report.errors.is_empty() && builds_removed {
        0
    } else {
        1
    }
}

#[tauri::command]
pub(crate) fn latex_environment_plan() -> LatexEnvironmentPlan {
    plan()
}

#[tauri::command]
pub(crate) async fn latex_environment_status(
    app: AppHandle,
) -> Result<LatexEnvironmentStatus, ServiceError> {
    let root = runtime_root(&app)?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let host = handle.state::<LatexRuntimeHost>();
        {
            let state = host.0.lock().map_err(|_| poison_err())?;
            if let Some(job) = &state.job {
                let progress = job.progress.lock().map_err(|_| poison_err())?.clone();
                return Ok(LatexEnvironmentStatus {
                    environment: state
                        .verified
                        .clone()
                        .unwrap_or(EnvironmentState::NotPrepared),
                    preparing: Some(PreparingView {
                        operation_id: job.operation_id.clone(),
                        progress,
                    }),
                    last: state.last.clone(),
                });
            }
            if let Some(verified) = &state.verified {
                return Ok(LatexEnvironmentStatus {
                    environment: verified.clone(),
                    preparing: None,
                    last: state.last.clone(),
                });
            }
        }
        // Hash outside the lock; a preparation cannot start meanwhile because
        // it would only invalidate `verified` again when it finishes.
        environment::clean_leftovers(&root);
        let verified = environment::verify(&root);
        let mut state = host.0.lock().map_err(|_| poison_err())?;
        state.verified = Some(verified.clone());
        Ok(LatexEnvironmentStatus {
            environment: verified,
            preparing: None,
            last: state.last.clone(),
        })
    })
    .await
    .map_err(map_join_err)?
}

#[tauri::command]
pub(crate) fn prepare_latex_environment(
    app: AppHandle,
    consent: bool,
    operation_id: String,
) -> Result<String, ServiceError> {
    // The plan must have been shown and accepted; no silent downloads.
    if !consent {
        return Err(svc_err(
            ErrorCode::InvalidInput,
            "error.latexConsentRequired",
        ));
    }
    if uuid::Uuid::parse_str(&operation_id).is_err() {
        return Err(svc_err(ErrorCode::InvalidInput, "error.invalidOperationId"));
    }
    let root = runtime_root(&app)?;
    let host = app.state::<LatexRuntimeHost>();
    let mut state = host.0.lock().map_err(|_| poison_err())?;
    if let Some(job) = &state.job {
        if job.operation_id == operation_id {
            return Ok(operation_id);
        }
        return Err(svc_err(ErrorCode::FileLocked, "error.latexPrepareRunning"));
    }
    // A build reads `active/`; it is not swapped out underneath it.
    if crate::latex_build::engine_busy() {
        return Err(svc_err(ErrorCode::FileLocked, "error.latexBuildRunning"));
    }
    let fixed = environment::fixed_manifest();
    let (deadline, download_deadline) = environment::deadlines(&fixed);
    let request = environment::PrepareRequest {
        operation_id: operation_id.clone(),
        engine: environment::EngineSource::Download,
        templates: preparation_templates(),
        prepare_profile: PREPARE_PROFILE.to_owned(),
        compile_profile: APP_PROFILE.to_owned(),
        deadline,
        download_deadline,
        max_resource_bytes: fixed.resources.max_bytes,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(Progress {
        stage: Stage::Downloading,
        bytes: 0,
        total_bytes: Some(fixed.engine.archive_bytes),
        step: 0,
        steps: 0,
    }));
    state.job = Some(Job {
        operation_id: operation_id.clone(),
        cancel: cancel.clone(),
        progress: progress.clone(),
    });
    let handle = app.clone();
    std::thread::spawn(move || {
        let outcome = environment::prepare(&root, &request, &cancel, &progress);
        let host = handle.state::<LatexRuntimeHost>();
        if let Ok(mut state) = host.0.lock() {
            state.job = None;
            state.verified = None;
            state.last = Some(PrepareOutcome {
                operation_id: request.operation_id.clone(),
                succeeded: outcome.is_ok(),
                reason_key: outcome
                    .as_ref()
                    .err()
                    .map(environment::PrepareError::reason_key),
            });
        };
    });
    Ok(operation_id)
}

#[tauri::command]
pub(crate) fn cancel_latex_preparation(app: AppHandle) -> Result<(), ServiceError> {
    let host = app.state::<LatexRuntimeHost>();
    let state = host.0.lock().map_err(|_| poison_err())?;
    if let Some(job) = &state.job {
        job.cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Removes the environment, both AppContainer profiles and the approved
/// profile-folder entries: the same steps as the uninstaller.
#[tauri::command]
pub(crate) async fn remove_latex_environment(
    app: AppHandle,
) -> Result<CleanupReport, ServiceError> {
    let root = runtime_root(&app)?;
    {
        let host = app.state::<LatexRuntimeHost>();
        let state = host.0.lock().map_err(|_| poison_err())?;
        if state.job.is_some() {
            return Err(svc_err(ErrorCode::FileLocked, "error.latexPrepareRunning"));
        }
    }
    if crate::latex_build::engine_busy() {
        return Err(svc_err(ErrorCode::FileLocked, "error.latexBuildRunning"));
    }
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = remove_everything(Some(&root), &PRODUCT_PROFILES);
        let host = handle.state::<LatexRuntimeHost>();
        let mut state = host.0.lock().map_err(|_| poison_err())?;
        state.verified = None;
        state.last = None;
        Ok(report)
    })
    .await
    .map_err(map_join_err)?
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn icacls(path: &Path) -> String {
        let output = std::process::Command::new("icacls")
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn profile_exists(profile: &str) -> bool {
        let output = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppContainer\Mappings",
                "/s",
                "/f",
                profile,
                "/d",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).contains(profile)
    }

    use environment::{EngineSource, PrepareError, PrepareRequest};

    /// Throwaway profiles for one end-to-end test, cleaned like an uninstall.
    struct Profiles(String, String);

    impl Profiles {
        fn new(tag: &str) -> Self {
            let id = std::process::id();
            Self(
                format!("opennote.sandboxtest.e2e.{tag}.prep.{id}"),
                format!("opennote.sandboxtest.e2e.{tag}.comp.{id}"),
            )
        }
    }

    impl Drop for Profiles {
        fn drop(&mut self) {
            let report = remove_everything(None, &[self.0.as_str(), self.1.as_str()]);
            if !report.errors.is_empty() {
                eprintln!("CLEANUP ERRORS {:?}", report.errors);
            }
        }
    }

    fn e2e_request(profiles: &Profiles, engine: EngineSource) -> PrepareRequest {
        let fixed = environment::fixed_manifest();
        let (deadline, download_deadline) = environment::deadlines(&fixed);
        PrepareRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            engine,
            templates: preparation_templates(),
            prepare_profile: profiles.0.clone(),
            compile_profile: profiles.1.clone(),
            deadline,
            download_deadline,
            max_resource_bytes: fixed.resources.max_bytes,
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

    fn only_entries(root: &Path) -> Vec<String> {
        std::fs::read_dir(root)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The local copy of the verified archive, to spare repeated downloads in
    /// the failure-path tests (its hash is checked like a download's).
    fn local_archive() -> EngineSource {
        EngineSource::File(PathBuf::from(
            std::env::var_os("ONOTE_TECTONIC_ARCHIVE").expect("set ONOTE_TECTONIC_ARCHIVE"),
        ))
    }

    #[test]
    #[ignore = "needs internet access"]
    fn a_real_preparation_activates_a_verified_offline_environment() {
        let profiles = Profiles::new("ok");
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("latex-runtime");
        let started = std::time::Instant::now();
        let manifest = environment::prepare(
            &root,
            &e2e_request(&profiles, EngineSource::Download),
            &AtomicBool::new(false),
            &idle(),
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        let cache: u64 = manifest
            .files
            .iter()
            .filter(|file| file.path.starts_with("cache/"))
            .map(|file| file.size)
            .sum();
        eprintln!(
            "E2E seconds={} totalBytes={} cacheBytes={cache} files={}",
            started.elapsed().as_secs(),
            manifest.total_bytes,
            manifest.files.len()
        );
        assert_eq!(only_entries(&root), vec!["active".to_owned()]);
        assert!(matches!(
            environment::verify(&root),
            EnvironmentState::Ready { .. }
        ));
        assert!(cache <= environment::fixed_manifest().resources.max_bytes);
    }

    #[test]
    #[ignore = "needs internet access and ONOTE_TECTONIC_ARCHIVE"]
    fn a_preparation_past_its_deadline_activates_nothing() {
        let profiles = Profiles::new("late");
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("latex-runtime");
        let mut request = e2e_request(&profiles, local_archive());
        request.deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let outcome = environment::prepare(&root, &request, &AtomicBool::new(false), &idle());
        assert_eq!(outcome.unwrap_err(), PrepareError::TimedOut);
        assert!(only_entries(&root).is_empty(), "{:?}", only_entries(&root));
        assert_eq!(environment::verify(&root), EnvironmentState::NotPrepared);
    }

    #[test]
    #[ignore = "needs internet access and ONOTE_TECTONIC_ARCHIVE"]
    fn a_preparation_over_its_download_limit_activates_nothing() {
        let profiles = Profiles::new("big");
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("latex-runtime");
        let mut request = e2e_request(&profiles, local_archive());
        request.max_resource_bytes = 1024 * 1024;
        let outcome = environment::prepare(&root, &request, &AtomicBool::new(false), &idle());
        assert_eq!(outcome.unwrap_err(), PrepareError::TooLarge);
        assert!(only_entries(&root).is_empty(), "{:?}", only_entries(&root));
    }

    #[test]
    #[ignore = "needs internet access and ONOTE_TECTONIC_ARCHIVE"]
    fn cancelling_while_fetching_activates_nothing() {
        let profiles = Profiles::new("cancel");
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("latex-runtime");
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(idle());
        let (watch_cancel, watch_progress) = (cancel.clone(), progress.clone());
        let watcher = std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let fetching = watch_progress.lock().unwrap().stage == Stage::FetchingResources;
            if fetching {
                std::thread::sleep(std::time::Duration::from_secs(2));
                watch_cancel.store(true, Ordering::Relaxed);
                return;
            }
        });
        let outcome = environment::prepare(
            &root,
            &e2e_request(&profiles, local_archive()),
            &cancel,
            &progress,
        );
        watcher.join().unwrap();
        assert_eq!(outcome.unwrap_err(), PrepareError::Cancelled);
        assert!(only_entries(&root).is_empty(), "{:?}", only_entries(&root));
    }

    #[test]
    fn the_preparation_covers_every_template_in_both_languages_and_the_samples() {
        let templates = preparation_templates();
        assert_eq!(templates.len(), 6 + 13);
        let names: std::collections::HashSet<_> =
            templates.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names.len(), templates.len());
        for name in [
            "sample-book-10pt",
            "sample-beamer",
            "sample-packages",
            "sample-ctex",
        ] {
            assert!(names.contains(name), "{name}");
        }
        assert!(templates
            .iter()
            .all(|(_, files)| files.iter().any(|(path, _)| path == "main.tex")));
    }

    #[test]
    fn the_plan_names_the_hosts_and_sizes_shown_before_consent() {
        let plan = plan();
        assert_eq!(plan.engine_host, "github.com");
        assert_eq!(plan.resource_host, "relay.fullyjustified.net");
        assert_eq!(plan.engine_download_bytes, 21_060_223);
        assert!(plan.resource_max_bytes >= plan.resource_estimated_bytes);
    }

    /// The uninstall cleanup removes the approved entries, the profile and the
    /// runtime directory, and is safe to run twice.
    #[test]
    fn cleanup_revokes_the_profile_folder_entries_and_deletes_everything() {
        let profile = format!("opennote.sandboxtest.cleanup.{}", std::process::id());
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("latex-runtime");
        let work = root.join("active").join("cache");
        std::fs::create_dir_all(&work).unwrap();
        // A real run applies the approved entries for this profile.
        let spec = onote_latex_sandbox::SandboxSpec {
            profile: profile.clone(),
            program: PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            args: vec!["/c".into(), "exit".into(), "0".into()],
            cwd: work.clone(),
            read_dirs: Vec::new(),
            write_dirs: vec![work.clone()],
            attribute_only_dirs: onote_latex_sandbox::profile_folders().unwrap(),
            timeout: std::time::Duration::from_secs(20),
            max_output_bytes: 1024,
            env: Vec::new(),
            network: onote_latex_sandbox::Network::None,
            byte_budget: None,
        };
        let run = onote_latex_sandbox::run(&spec, &AtomicBool::new(false)).unwrap();
        assert_eq!(run.exit, onote_latex_sandbox::Exit::Exited(0));
        let sid = onote_latex_sandbox::profile_sid(&profile).unwrap();
        let folders = onote_latex_sandbox::profile_folders().unwrap();
        for folder in &folders {
            assert!(icacls(folder).contains(&sid), "entry was not applied");
        }
        assert!(profile_exists(&profile));

        let report = remove_everything(Some(&root), &[profile.as_str()]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.runtime_removed);
        for folder in &folders {
            assert!(
                !icacls(folder).contains(&sid),
                "entry left on {}",
                folder.display()
            );
        }
        assert!(!profile_exists(&profile));
        assert!(!root.exists());

        // Twice is fine: nothing left is not an error.
        let again = remove_everything(Some(&root), &[profile.as_str()]);
        assert!(again.errors.is_empty(), "{:?}", again.errors);
    }
}
