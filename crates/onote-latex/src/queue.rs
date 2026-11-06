//! M13 compile queue state machine (F51).
//!
//! This module models *when* a compile may run and *what state a job is in*. It
//! never spawns a process: the process belongs to the managed environment, and
//! keeping the queue free of `std::process` is what lets the state rules be tested
//! exhaustively.
//!
//! The rules that must not be re-derived by a caller:
//!
//! * one job runs per project at a time, and queued requests collapse to the
//!   newest snapshot rather than piling up (F51-R01, F51-R02);
//! * a request that arrives while a job runs becomes the *next* job with an
//!   immutable input, so a running build can never change underneath itself;
//! * a job left `Running` by a previous process is reported as interrupted, never
//!   as succeeded — a partial PDF is not a result (F51-R03);
//! * a successful artefact whose source hash no longer matches is `Outdated`
//!   (F51-R04).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Default timeout from F51 §4.
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// The idle delay before an automatic compile starts (F51 §4).
pub const AUTO_COMPILE_DELAY_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Preparing,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    /// Found `Running` at startup: the process is gone, so the result cannot be
    /// trusted (F51-R03).
    Interrupted,
}

impl JobState {
    /// States a job can no longer leave.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::TimedOut | Self::Interrupted
        )
    }

    /// True only for the state that means "there is a trustworthy result".
    pub fn is_success(self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// The immutable input a job was created with (F51-R02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInput {
    /// Hash of the manifest at request time.
    pub manifest_hash: String,
    /// Hash of the source snapshot the job will compile.
    pub source_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub job_id: String,
    pub project_id: String,
    pub state: JobState,
    /// Set when the job was accepted, so a later request cannot change it.
    pub input: JobInput,
    /// The request that created it, for idempotent replay.
    pub operation_id: String,
    /// Artefact id when the job succeeded.
    pub artifact_id: Option<String>,
    /// Why the job ended, for a failed or interrupted job.
    pub reason_key: Option<String>,
}

/// Why a compile request was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueError {
    /// The manifest changed since the caller read it.
    ManifestMismatch { expected: String, actual: String },
    /// No such job.
    UnknownJob(String),
    /// The job already reached a terminal state, so it cannot be cancelled.
    AlreadyFinished(JobState),
    /// The requested transition is not possible: a queued job cannot succeed
    /// or fail before it starts, and `finish` cannot move a job to a
    /// non-terminal state.
    InvalidTransition { from: JobState, to: JobState },
    /// The operation id was already used for a different project or a different
    /// input snapshot. Replaying it would report a job that compiled something
    /// else as if it were this request (F51 §6).
    OperationConflict {
        operation_id: String,
        existing_job_id: String,
    },
}

/// The outcome of asking for a compile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileOutcome {
    /// The job the caller should watch.
    pub job_id: String,
    pub state: JobState,
    /// True when this call created the job; false when it replayed a known
    /// operation id or an identical queued request.
    pub created: bool,
}

/// The queue for one or more projects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    jobs: Vec<Job>,
    /// The job currently running, per project (F51-R01).
    running: BTreeMap<String, String>,
    /// The pending job per project; a newer request replaces it (F51-R02).
    queued: BTreeMap<String, String>,
    /// Monotonic counter, so ids are deterministic in tests.
    next_id: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn job(&self, job_id: &str) -> Option<&Job> {
        self.jobs.iter().find(|job| job.job_id == job_id)
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    /// The job currently running for a project, if any.
    pub fn running_job(&self, project_id: &str) -> Option<&Job> {
        self.running
            .get(project_id)
            .and_then(|job_id| self.job(job_id))
    }

    /// The pending job for a project, if any.
    pub fn queued_job(&self, project_id: &str) -> Option<&Job> {
        self.queued
            .get(project_id)
            .and_then(|job_id| self.job(job_id))
    }

    /// Requests a compile.
    ///
    /// The manifest hash is checked first: a stale expectation means the caller
    /// read an older manifest, and compiling it would build the wrong document.
    pub fn request(
        &mut self,
        project_id: &str,
        expected_manifest_hash: &str,
        actual_manifest_hash: &str,
        input: JobInput,
        operation_id: &str,
    ) -> Result<CompileOutcome, QueueError> {
        if expected_manifest_hash != actual_manifest_hash {
            return Err(QueueError::ManifestMismatch {
                expected: expected_manifest_hash.to_owned(),
                actual: actual_manifest_hash.to_owned(),
            });
        }
        // The same operation id replays instead of creating a second job, but
        // only for the request it was created from. Reusing an id for another
        // project or snapshot would return a job that built something else.
        if let Some(existing) = self
            .jobs
            .iter()
            .find(|job| job.operation_id == operation_id)
        {
            if existing.project_id != project_id || existing.input != input {
                return Err(QueueError::OperationConflict {
                    operation_id: operation_id.to_owned(),
                    existing_job_id: existing.job_id.clone(),
                });
            }
            return Ok(CompileOutcome {
                job_id: existing.job_id.clone(),
                state: existing.state,
                created: false,
            });
        }
        // A running job owns the project; the new request becomes the single
        // pending job, replacing any earlier pending one.
        if self.running.contains_key(project_id) {
            if let Some(existing_id) = self.queued.get(project_id).cloned() {
                if let Some(existing) = self.jobs.iter_mut().find(|job| job.job_id == existing_id) {
                    if existing.input == input {
                        // Waiting for exactly this snapshot already: do not queue
                        // a duplicate, which would build the same thing twice.
                        return Ok(CompileOutcome {
                            job_id: existing.job_id.clone(),
                            state: existing.state,
                            created: false,
                        });
                    }
                    // Superseded, so it ends here with an explicit state rather
                    // than lingering as a stale pending job.
                    existing.state = JobState::Cancelled;
                    existing.reason_key = Some("build.superseded".to_owned());
                }
            }
            let job = self.push_job(project_id, input, operation_id, JobState::Queued);
            self.queued
                .insert(project_id.to_owned(), job.job_id.clone());
            return Ok(CompileOutcome {
                job_id: job.job_id,
                state: JobState::Queued,
                created: true,
            });
        }
        let job = self.push_job(project_id, input, operation_id, JobState::Running);
        self.running
            .insert(project_id.to_owned(), job.job_id.clone());
        Ok(CompileOutcome {
            job_id: job.job_id,
            state: JobState::Running,
            created: true,
        })
    }

    fn push_job(
        &mut self,
        project_id: &str,
        input: JobInput,
        operation_id: &str,
        state: JobState,
    ) -> Job {
        self.next_id += 1;
        let job = Job {
            job_id: format!("job-{}", self.next_id),
            project_id: project_id.to_owned(),
            state,
            input,
            operation_id: operation_id.to_owned(),
            artifact_id: None,
            reason_key: None,
        };
        self.jobs.push(job.clone());
        job
    }

    /// Marks a running job succeeded and starts the pending one, if any.
    pub fn finish_success(
        &mut self,
        job_id: &str,
        artifact_id: &str,
        artifact_source_hash: &str,
        current_source_hash: &str,
    ) -> Result<JobState, QueueError> {
        let project_id = self.finish(job_id, JobState::Succeeded, None)?;
        if let Some(job) = self.jobs.iter_mut().find(|job| job.job_id == job_id) {
            job.artifact_id = Some(artifact_id.to_owned());
            // F51-R04: a result built from a snapshot that is no longer current is
            // reported as outdated rather than presented as the latest build.
            if artifact_source_hash != current_source_hash {
                job.reason_key = Some("build.outdated".to_owned());
            }
        }
        self.start_next(&project_id);
        Ok(JobState::Succeeded)
    }

    /// Marks a running job failed, timed out or cancelled.
    ///
    /// A cancellation clears unpublished output but never touches the source, and
    /// the previous successful artefact stays (BUILD_CANCELLED).
    pub fn finish(
        &mut self,
        job_id: &str,
        state: JobState,
        reason_key: Option<&str>,
    ) -> Result<String, QueueError> {
        let (project_id, current) = {
            let job = self
                .jobs
                .iter()
                .find(|job| job.job_id == job_id)
                .ok_or_else(|| QueueError::UnknownJob(job_id.to_owned()))?;
            (job.project_id.clone(), job.state)
        };
        if current.is_terminal() {
            return Err(QueueError::AlreadyFinished(current));
        }
        // Only a job that actually started can succeed, fail or time out. A
        // queued job may be cancelled, but it must never be recorded as having
        // produced a result. `Interrupted` is reserved for `recover`.
        let started = matches!(current, JobState::Running | JobState::Preparing);
        let target_allowed = matches!(
            state,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled | JobState::TimedOut
        );
        if !target_allowed || (state != JobState::Cancelled && !started) {
            return Err(QueueError::InvalidTransition {
                from: current,
                to: state,
            });
        }
        if let Some(job) = self.jobs.iter_mut().find(|job| job.job_id == job_id) {
            job.state = state;
            job.reason_key = reason_key.map(str::to_owned);
            // A cancelled job publishes nothing.
            if state != JobState::Succeeded {
                job.artifact_id = None;
            }
        }
        if self.running.get(&project_id) == Some(&job_id.to_owned()) {
            self.running.remove(&project_id);
        }
        if self.queued.get(&project_id) == Some(&job_id.to_owned()) {
            self.queued.remove(&project_id);
        }
        Ok(project_id)
    }

    /// Starts the pending job for a project, if the project is now free.
    fn start_next(&mut self, project_id: &str) {
        if self.running.contains_key(project_id) {
            return;
        }
        let Some(pending) = self.queued.remove(project_id) else {
            return;
        };
        if let Some(job) = self.jobs.iter_mut().find(|job| job.job_id == pending) {
            if job.state == JobState::Queued {
                // The pending job starts with its own immutable input (F51-R02).
                job.state = JobState::Running;
                self.running.insert(project_id.to_owned(), pending);
            }
        }
    }

    /// Starts the pending job after an explicit cancel or failure.
    pub fn start_pending(&mut self, project_id: &str) -> Option<String> {
        self.start_next(project_id);
        self.running.get(project_id).cloned()
    }

    /// Marks jobs left `Running` by a previous process as interrupted (F51-R03).
    ///
    /// Their artefacts are dropped: a PDF written by a killed process is not a
    /// verified result, so treating it as success would be a silent lie.
    pub fn recover(&mut self) -> Vec<String> {
        let mut interrupted = Vec::new();
        for job in self.jobs.iter_mut() {
            if matches!(job.state, JobState::Running | JobState::Preparing) {
                job.state = JobState::Interrupted;
                job.reason_key = Some("build.interrupted".to_owned());
                job.artifact_id = None;
                interrupted.push(job.job_id.clone());
            }
        }
        self.running.clear();
        // A pending job survives a restart: it never started, so its input is
        // still valid and the user is waiting for it.
        interrupted
    }

    /// True when the queue has nothing running and nothing pending.
    pub fn is_idle(&self) -> bool {
        self.running.is_empty() && self.queued.is_empty()
    }
}

/// Whether an automatic compile should start now (F51 §4).
///
/// An automatic build only fires after a *complete* save and an idle period; a
/// compile during typing would build a document the user is still changing.
pub fn should_auto_compile(
    auto_compile: bool,
    saved_completely: bool,
    idle_ms: u64,
    has_running_job: bool,
) -> bool {
    auto_compile && saved_completely && idle_ms >= AUTO_COMPILE_DELAY_MS && !has_running_job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(hash: &str) -> JobInput {
        JobInput {
            manifest_hash: "m1".to_owned(),
            source_hash: hash.to_owned(),
        }
    }

    #[test]
    fn one_job_runs_per_project_at_a_time() {
        // F51-R01.
        let mut queue = Queue::new();
        let first = queue
            .request("p1", "m1", "m1", input("s1"), "op1")
            .expect("first");
        assert_eq!(first.state, JobState::Running);
        let second = queue
            .request("p1", "m1", "m1", input("s2"), "op2")
            .expect("second");
        assert_eq!(second.state, JobState::Queued);
        assert_eq!(
            queue.running_job("p1").map(|job| job.job_id.clone()),
            Some(first.job_id)
        );
    }

    #[test]
    fn two_projects_can_run_concurrently() {
        let mut queue = Queue::new();
        let a = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("a");
        let b = queue
            .request("p2", "m", "m", input("s2"), "op2")
            .expect("b");
        assert_eq!(a.state, JobState::Running);
        assert_eq!(b.state, JobState::Running);
    }

    #[test]
    fn a_newer_request_replaces_the_pending_one() {
        // F51-R02: waiting work collapses to the newest snapshot.
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        let pending = queue
            .request("p1", "m", "m", input("s2"), "op2")
            .expect("pending");
        let newer = queue
            .request("p1", "m", "m", input("s3"), "op3")
            .expect("newer");
        assert!(newer.created);
        assert_eq!(
            queue.queued_job("p1").map(|job| job.job_id.clone()),
            Some(newer.job_id.clone())
        );
        // The superseded job says so instead of lingering silently.
        let superseded = queue.job(&pending.job_id).expect("superseded");
        assert_eq!(superseded.state, JobState::Cancelled);
        assert_eq!(superseded.reason_key.as_deref(), Some("build.superseded"));
        assert_eq!(
            queue.job(&running.job_id).map(|job| job.state),
            Some(JobState::Running)
        );
    }

    #[test]
    fn an_identical_pending_request_is_not_duplicated() {
        let mut queue = Queue::new();
        queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        let first = queue
            .request("p1", "m", "m", input("s2"), "op2")
            .expect("pending");
        let again = queue
            .request("p1", "m", "m", input("s2"), "op3")
            .expect("again");
        assert!(!again.created);
        assert_eq!(again.job_id, first.job_id);
    }

    #[test]
    fn the_running_input_never_changes() {
        // F51-R02: a running job's input is immutable.
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        queue
            .request("p1", "m", "m", input("s2"), "op2")
            .expect("pending");
        assert_eq!(
            queue
                .job(&running.job_id)
                .map(|job| job.input.source_hash.clone()),
            Some("s1".to_owned())
        );
    }

    #[test]
    fn a_stale_manifest_expectation_is_refused() {
        let mut queue = Queue::new();
        let error = queue
            .request("p1", "old", "new", input("s1"), "op1")
            .expect_err("must refuse");
        assert_eq!(
            error,
            QueueError::ManifestMismatch {
                expected: "old".to_owned(),
                actual: "new".to_owned()
            }
        );
        assert!(queue.jobs().is_empty());
    }

    #[test]
    fn the_same_operation_id_replays_instead_of_creating_a_job() {
        let mut queue = Queue::new();
        let first = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("first");
        let replay = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("replay");
        assert!(!replay.created);
        assert_eq!(replay.job_id, first.job_id);
        assert_eq!(queue.jobs().len(), 1);
    }

    #[test]
    fn the_same_operation_id_with_a_different_snapshot_is_a_conflict() {
        // F51 §6: reusing an operation id after the baseline changed is a typed
        // conflict, not a replay of a job that built the old snapshot.
        let mut queue = Queue::new();
        let first = queue
            .request("p1", "m1", "m1", input("s1"), "op1")
            .expect("first");
        let conflict = queue.request("p1", "m1", "m1", input("s2"), "op1");
        assert_eq!(
            conflict,
            Err(QueueError::OperationConflict {
                operation_id: "op1".to_owned(),
                existing_job_id: first.job_id.clone(),
            })
        );
        assert_eq!(queue.jobs().len(), 1);
        assert_eq!(
            queue.running_job("p1").map(|job| job.input.clone()),
            Some(input("s1"))
        );
    }

    #[test]
    fn the_same_operation_id_cannot_compile_another_project() {
        let mut queue = Queue::new();
        let first = queue
            .request("p1", "m1", "m1", input("s1"), "op1")
            .expect("first");
        let conflict = queue.request("p2", "m1", "m1", input("s1"), "op1");
        assert!(matches!(
            conflict,
            Err(QueueError::OperationConflict { .. })
        ));
        assert_eq!(queue.jobs().len(), 1);
        assert!(queue.running_job("p2").is_none());
        assert_eq!(
            queue.running_job("p1").map(|job| job.job_id.clone()),
            Some(first.job_id)
        );
    }

    #[test]
    fn replaying_a_finished_operation_returns_the_same_terminal_job() {
        // The same request may be retried after it finished; it must not create
        // a second compile or forget the artifact (F51 §6).
        let mut queue = Queue::new();
        let first = queue
            .request("p1", "m1", "m1", input("s1"), "op1")
            .expect("first");
        queue
            .finish_success(&first.job_id, "a1", "s1", "s1")
            .expect("success");
        let replay = queue
            .request("p1", "m1", "m1", input("s1"), "op1")
            .expect("replay");
        assert!(!replay.created);
        assert_eq!(replay.state, JobState::Succeeded);
        assert_eq!(queue.jobs().len(), 1);
        assert_eq!(
            queue
                .job(&first.job_id)
                .and_then(|job| job.artifact_id.clone()),
            Some("a1".to_owned())
        );
    }

    #[test]
    fn success_marks_the_result_and_starts_the_pending_job() {
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        let pending = queue
            .request("p1", "m", "m", input("s2"), "op2")
            .expect("pending");
        queue
            .finish_success(&running.job_id, "a1", "s1", "s1")
            .expect("success");
        assert_eq!(
            queue.job(&running.job_id).map(|job| job.state),
            Some(JobState::Succeeded)
        );
        assert_eq!(
            queue
                .job(&running.job_id)
                .and_then(|job| job.artifact_id.clone()),
            Some("a1".to_owned())
        );
        // The pending job starts as soon as the project is free.
        assert_eq!(
            queue.running_job("p1").map(|job| job.job_id.clone()),
            Some(pending.job_id)
        );
    }

    #[test]
    fn a_result_from_an_older_snapshot_is_marked_outdated() {
        // F51-R04.
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        queue
            .finish_success(&running.job_id, "a1", "s1", "s2")
            .expect("success");
        let job = queue.job(&running.job_id).expect("job");
        assert_eq!(job.state, JobState::Succeeded);
        assert_eq!(job.reason_key.as_deref(), Some("build.outdated"));
    }

    #[test]
    fn cancelling_clears_output_but_not_the_previous_artefact() {
        // BUILD_CANCELLED: only unpublished output is cleaned.
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        queue
            .finish(
                &running.job_id,
                JobState::Cancelled,
                Some("build.cancelled"),
            )
            .expect("cancel");
        let job = queue.job(&running.job_id).expect("job");
        assert_eq!(job.state, JobState::Cancelled);
        assert_eq!(job.artifact_id, None);
        assert!(queue.is_idle());
    }

    #[test]
    fn a_timeout_is_a_terminal_state_with_a_reason() {
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        queue
            .finish(&running.job_id, JobState::TimedOut, Some("build.timedOut"))
            .expect("timeout");
        let job = queue.job(&running.job_id).expect("job");
        assert!(job.state.is_terminal());
        assert!(!job.state.is_success());
        assert_eq!(job.reason_key.as_deref(), Some("build.timedOut"));
    }

    #[test]
    fn a_finished_job_cannot_be_finished_again() {
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        queue
            .finish_success(&running.job_id, "a1", "s1", "s1")
            .expect("success");
        assert_eq!(
            queue.finish(&running.job_id, JobState::Failed, None),
            Err(QueueError::AlreadyFinished(JobState::Succeeded))
        );
    }

    #[test]
    fn a_queued_job_cannot_be_recorded_as_succeeded() {
        // A job that never started cannot have produced an artefact, even if a
        // caller claims it did (F51-R06, "no false success").
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        let pending = queue
            .request("p1", "m", "m", input("s2"), "op2")
            .expect("pending");
        assert_eq!(
            queue.finish_success(&pending.job_id, "forged", "s2", "s2"),
            Err(QueueError::InvalidTransition {
                from: JobState::Queued,
                to: JobState::Succeeded,
            })
        );
        let job = queue.job(&pending.job_id).expect("pending job");
        assert_eq!(job.state, JobState::Queued);
        assert_eq!(job.artifact_id, None);
        assert_eq!(
            queue.running_job("p1").map(|job| job.job_id.clone()),
            Some(running.job_id)
        );
    }

    #[test]
    fn a_queued_job_can_still_be_cancelled() {
        // F51 §6 exposes cancel(jobId); a pending request must remain
        // cancellable even though it never ran.
        let mut queue = Queue::new();
        queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        let pending = queue
            .request("p1", "m", "m", input("s2"), "op2")
            .expect("pending");
        queue
            .finish(
                &pending.job_id,
                JobState::Cancelled,
                Some("build.cancelled"),
            )
            .expect("cancel");
        assert_eq!(
            queue.job(&pending.job_id).map(|job| job.state),
            Some(JobState::Cancelled)
        );
        assert!(queue.queued_job("p1").is_none());
        // The running job is unaffected, so the project is not idle.
        assert!(!queue.is_idle());
    }

    #[test]
    fn finish_cannot_move_a_job_to_a_non_terminal_state() {
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        for target in [
            JobState::Queued,
            JobState::Running,
            JobState::Preparing,
            JobState::Interrupted,
        ] {
            assert_eq!(
                queue.finish(&running.job_id, target, None),
                Err(QueueError::InvalidTransition {
                    from: JobState::Running,
                    to: target,
                })
            );
        }
        assert_eq!(
            queue.job(&running.job_id).map(|job| job.state),
            Some(JobState::Running)
        );
    }

    #[test]
    fn an_unknown_job_is_reported() {
        let mut queue = Queue::new();
        assert_eq!(
            queue.finish("nope", JobState::Failed, None),
            Err(QueueError::UnknownJob("nope".to_owned()))
        );
    }

    #[test]
    fn recovery_marks_a_leftover_running_job_interrupted() {
        // F51-R03: a killed process must not leave a "successful" PDF behind.
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        let interrupted = queue.recover();
        assert_eq!(interrupted, vec![running.job_id.clone()]);
        let job = queue.job(&running.job_id).expect("job");
        assert_eq!(job.state, JobState::Interrupted);
        assert_eq!(job.artifact_id, None);
        assert_eq!(job.reason_key.as_deref(), Some("build.interrupted"));
        assert!(queue.is_idle());
    }

    #[test]
    fn recovery_keeps_a_pending_job_and_can_start_it() {
        let mut queue = Queue::new();
        queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        let pending = queue
            .request("p1", "m", "m", input("s2"), "op2")
            .expect("pending");
        queue.recover();
        assert_eq!(
            queue.job(&pending.job_id).map(|job| job.state),
            Some(JobState::Queued),
            "a job that never started keeps its state"
        );
        let started = queue.start_pending("p1");
        assert_eq!(started, Some(pending.job_id.clone()));
        assert_eq!(
            queue.job(&pending.job_id).map(|job| job.state),
            Some(JobState::Running)
        );
    }

    #[test]
    fn a_failed_job_also_releases_the_project() {
        let mut queue = Queue::new();
        let running = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("running");
        queue
            .finish(&running.job_id, JobState::Failed, Some("build.failed"))
            .expect("fail");
        assert!(queue.running_job("p1").is_none());
        let next = queue
            .request("p1", "m", "m", input("s2"), "op2")
            .expect("next");
        assert_eq!(next.state, JobState::Running);
    }

    #[test]
    fn automatic_compile_needs_a_complete_save_and_idle_time() {
        // F51 §4: never build while the user is still typing.
        assert!(should_auto_compile(
            true,
            true,
            AUTO_COMPILE_DELAY_MS,
            false
        ));
        assert!(!should_auto_compile(
            false,
            true,
            AUTO_COMPILE_DELAY_MS,
            false
        ));
        assert!(!should_auto_compile(
            true,
            false,
            AUTO_COMPILE_DELAY_MS,
            false
        ));
        assert!(!should_auto_compile(
            true,
            true,
            AUTO_COMPILE_DELAY_MS - 1,
            false
        ));
        assert!(!should_auto_compile(
            true,
            true,
            AUTO_COMPILE_DELAY_MS,
            true
        ));
    }

    #[test]
    fn job_ids_are_unique_and_ordered() {
        let mut queue = Queue::new();
        let a = queue
            .request("p1", "m", "m", input("s1"), "op1")
            .expect("a");
        let b = queue
            .request("p2", "m", "m", input("s2"), "op2")
            .expect("b");
        assert_ne!(a.job_id, b.job_id);
        assert_eq!(a.job_id, "job-1");
        assert_eq!(b.job_id, "job-2");
    }
}
