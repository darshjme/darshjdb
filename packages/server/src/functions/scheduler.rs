//! Cron-based function scheduler with distributed locking and retry.
//!
//! Runs a background loop that checks for due scheduled jobs every 10 seconds,
//! acquires a Postgres advisory lock to prevent duplicate execution across
//! instances, and retries failures with exponential backoff (up to 3 attempts).
//!
//! # Usage
//!
//! ```rust,no_run
//! use darshandb_server::functions::scheduler::Scheduler;
//!
//! # async fn example(pool: sqlx::PgPool) -> anyhow::Result<()> {
//! let scheduler = Scheduler::new(pool);
//! scheduler.register_job(
//!     "cleanup:expiredSessions",
//!     "0 */15 * * * *",  // every 15 minutes
//! )?;
//! scheduler.start().await;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from the scheduler subsystem.
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// The cron expression could not be parsed.
    #[error("invalid cron expression `{expr}`: {reason}")]
    InvalidCron {
        /// The cron expression that failed.
        expr: String,
        /// Parse error description.
        reason: String,
    },

    /// A database operation failed.
    #[error("scheduler database error: {0}")]
    Database(#[from] sqlx::Error),

    /// A job with this name already exists.
    #[error("duplicate job name: {0}")]
    DuplicateJob(String),

    /// The referenced job was not found.
    #[error("job not found: {0}")]
    JobNotFound(String),

    /// The scheduler is already running.
    #[error("scheduler is already running")]
    AlreadyRunning,
}

/// Result alias for scheduler operations.
pub type SchedulerResult<T> = std::result::Result<T, SchedulerError>;

// ---------------------------------------------------------------------------
// Job definition
// ---------------------------------------------------------------------------

/// Execution status of a scheduled job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    /// Waiting for the next scheduled time.
    Idle,
    /// Currently executing.
    Running,
    /// Last execution failed, awaiting retry.
    Retrying,
    /// Permanently disabled (e.g. after max retries on a non-recoverable error).
    Disabled,
}

/// A scheduled function job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    /// Unique job identifier (typically the fully qualified function name).
    pub id: String,

    /// Fully qualified function name to invoke (e.g. `"crons:cleanupSessions"`).
    pub function_name: String,

    /// Cron expression (6-field with seconds, e.g. `"0 */15 * * * *"`).
    pub cron_expr: String,

    /// Next scheduled execution time.
    pub next_run_at: Option<DateTime<Utc>>,

    /// When the job last ran (regardless of success/failure).
    pub last_run_at: Option<DateTime<Utc>>,

    /// Current status.
    pub status: JobStatus,

    /// Number of consecutive failures (resets on success).
    pub consecutive_failures: u32,

    /// Maximum retry attempts before disabling.
    pub max_retries: u32,
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// Background cron scheduler for DarshanDB server functions.
///
/// Uses Postgres advisory locks for distributed safety so only one server
/// instance executes a given job even in a multi-node deployment.
pub struct Scheduler {
    /// Postgres connection pool for advisory locks and job state.
    pool: PgPool,

    /// Registered jobs keyed by job ID.
    jobs: Arc<RwLock<HashMap<String, ScheduledJob>>>,

    /// Handle to the background tick task.
    task_handle: Arc<RwLock<Option<JoinHandle<()>>>>,

    /// Callback invoked when a job fires. In production this calls
    /// [`FunctionRuntime::execute`]; tests can substitute a mock.
    executor: Arc<dyn JobExecutor>,
}

/// Trait for the callback that actually runs a scheduled function.
///
/// Separated from the scheduler to allow testing without a full runtime.
pub trait JobExecutor: Send + Sync + 'static {
    /// Execute the function identified by `function_name`.
    /// Returns `Ok(())` on success or an error message on failure.
    fn execute_job(
        &self,
        function_name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
}

/// No-op executor used as a placeholder until the real runtime is wired in.
pub struct NoopExecutor;

impl JobExecutor for NoopExecutor {
    fn execute_job(
        &self,
        function_name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let name = function_name.to_string();
        Box::pin(async move {
            warn!(function_name = %name, "noop executor: job would have been executed");
            Ok(())
        })
    }
}

impl Scheduler {
    /// Create a new scheduler with the default no-op executor.
    ///
    /// Call [`Self::with_executor`] to provide a real function runtime.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            task_handle: Arc::new(RwLock::new(None)),
            executor: Arc::new(NoopExecutor),
        }
    }

    /// Create a scheduler with a custom job executor.
    pub fn with_executor(pool: PgPool, executor: Arc<dyn JobExecutor>) -> Self {
        Self {
            pool,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            task_handle: Arc::new(RwLock::new(None)),
            executor,
        }
    }

    /// Register a new scheduled job.
    ///
    /// Parses the cron expression and computes the first `next_run_at`.
    pub fn register_job(&self, function_name: &str, cron_expr: &str) -> SchedulerResult<()> {
        let schedule = parse_cron(cron_expr)?;
        let next_run = schedule.upcoming(Utc).next();

        let job = ScheduledJob {
            id: function_name.to_string(),
            function_name: function_name.to_string(),
            cron_expr: cron_expr.to_string(),
            next_run_at: next_run,
            last_run_at: None,
            status: JobStatus::Idle,
            consecutive_failures: 0,
            max_retries: 3,
        };

        // Use blocking write since register is called during startup.
        let jobs = self.jobs.clone();
        let name = function_name.to_string();

        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut lock = jobs.write().await;
                if lock.contains_key(&name) {
                    return Err(SchedulerError::DuplicateJob(name));
                }
                info!(
                    job_id = %job.id,
                    cron = %job.cron_expr,
                    next_run = ?job.next_run_at,
                    "registered scheduled job"
                );
                lock.insert(job.id.clone(), job);
                Ok(())
            })
        })
    }

    /// Remove a scheduled job.
    pub async fn unregister_job(&self, job_id: &str) -> SchedulerResult<()> {
        let mut lock = self.jobs.write().await;
        lock.remove(job_id)
            .ok_or_else(|| SchedulerError::JobNotFound(job_id.to_string()))?;
        info!(job_id, "unregistered scheduled job");
        Ok(())
    }

    /// Return a snapshot of all registered jobs.
    pub async fn list_jobs(&self) -> Vec<ScheduledJob> {
        self.jobs.read().await.values().cloned().collect()
    }

    /// Start the background scheduler loop.
    ///
    /// Checks for due jobs every 10 seconds. Uses Postgres advisory locks
    /// to ensure distributed safety.
    pub async fn start(&self) -> SchedulerResult<()> {
        {
            let handle = self.task_handle.read().await;
            if handle.is_some() {
                return Err(SchedulerError::AlreadyRunning);
            }
        }

        let pool = self.pool.clone();
        let jobs = Arc::clone(&self.jobs);
        let executor = Arc::clone(&self.executor);

        let handle = tokio::spawn(async move {
            let tick_interval = Duration::from_secs(10);
            let mut interval = tokio::time::interval(tick_interval);
            // Skip the first immediate tick.
            interval.tick().await;

            info!("scheduler background loop started (10s tick)");

            loop {
                interval.tick().await;
                if let Err(e) = tick_once(&pool, &jobs, &executor).await {
                    error!(error = %e, "scheduler tick failed");
                }
            }
        });

        let mut lock = self.task_handle.write().await;
        *lock = Some(handle);
        Ok(())
    }

    /// Stop the background scheduler loop.
    pub async fn stop(&self) {
        let mut lock = self.task_handle.write().await;
        if let Some(handle) = lock.take() {
            handle.abort();
            info!("scheduler stopped");
        }
    }
}

// ---------------------------------------------------------------------------
// Tick logic
// ---------------------------------------------------------------------------

/// Run a single tick of the scheduler loop.
///
/// For each due job, attempts to acquire an advisory lock, execute the
/// function, and update the job state.
async fn tick_once(
    pool: &PgPool,
    jobs: &Arc<RwLock<HashMap<String, ScheduledJob>>>,
    executor: &Arc<dyn JobExecutor>,
) -> Result<(), SchedulerError> {
    let now = Utc::now();
    let due_jobs: Vec<String>;

    {
        let lock = jobs.read().await;
        due_jobs = lock
            .values()
            .filter(|j| j.status != JobStatus::Disabled && j.next_run_at.is_some_and(|t| t <= now))
            .map(|j| j.id.clone())
            .collect();
    }

    for job_id in due_jobs {
        let pool = pool.clone();
        let jobs = Arc::clone(jobs);
        let executor = Arc::clone(executor);

        // Spawn each job execution as a separate task for parallelism.
        tokio::spawn(async move {
            if let Err(e) = execute_job_with_lock(&pool, &jobs, &executor, &job_id).await {
                error!(job_id = %job_id, error = %e, "failed to execute scheduled job");
            }
        });
    }

    Ok(())
}

/// Attempt to acquire a Postgres advisory lock and execute a job.
///
/// The lock key is derived from a hash of the job ID to avoid collisions.
/// If the lock cannot be acquired (another instance is running this job),
/// the function returns immediately without error.
async fn execute_job_with_lock(
    pool: &PgPool,
    jobs: &Arc<RwLock<HashMap<String, ScheduledJob>>>,
    executor: &Arc<dyn JobExecutor>,
    job_id: &str,
) -> Result<(), SchedulerError> {
    let lock_key = advisory_lock_key(job_id);

    // Try to acquire the advisory lock (non-blocking).
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(lock_key)
        .fetch_one(pool)
        .await?;

    if !acquired {
        debug!(
            job_id,
            "advisory lock not acquired, skipping (another instance is running)"
        );
        return Ok(());
    }

    // Mark as running.
    {
        let mut lock = jobs.write().await;
        if let Some(job) = lock.get_mut(job_id) {
            job.status = JobStatus::Running;
        }
    }

    let function_name = {
        let lock = jobs.read().await;
        lock.get(job_id)
            .map(|j| j.function_name.clone())
            .ok_or_else(|| SchedulerError::JobNotFound(job_id.to_string()))?
    };

    // Execute with retry.
    let max_retries = {
        let lock = jobs.read().await;
        lock.get(job_id).map(|j| j.max_retries).unwrap_or(3)
    };

    let mut last_error = None;
    for attempt in 0..=max_retries {
        if attempt > 0 {
            // Exponential backoff: 1s, 2s, 4s.
            let backoff = Duration::from_secs(1 << (attempt - 1));
            debug!(
                job_id,
                attempt,
                backoff_secs = backoff.as_secs(),
                "retrying scheduled job"
            );
            tokio::time::sleep(backoff).await;
        }

        match executor.execute_job(&function_name).await {
            Ok(()) => {
                info!(job_id, attempt, "scheduled job completed successfully");
                last_error = None;
                break;
            }
            Err(e) => {
                warn!(job_id, attempt, error = %e, "scheduled job failed");
                last_error = Some(e);
            }
        }
    }

    // Update job state.
    {
        let mut lock = jobs.write().await;
        if let Some(job) = lock.get_mut(job_id) {
            let now = Utc::now();
            job.last_run_at = Some(now);

            if last_error.is_some() {
                job.consecutive_failures += 1;
                if job.consecutive_failures > max_retries {
                    job.status = JobStatus::Disabled;
                    error!(
                        job_id,
                        failures = job.consecutive_failures,
                        "scheduled job disabled after max retries"
                    );
                } else {
                    job.status = JobStatus::Retrying;
                }
            } else {
                job.consecutive_failures = 0;
                job.status = JobStatus::Idle;
            }

            // Compute next run time.
            if let Ok(schedule) = parse_cron(&job.cron_expr) {
                job.next_run_at = schedule.upcoming(Utc).next();
            }
        }
    }

    // Release the advisory lock.
    let _: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
        .bind(lock_key)
        .fetch_one(pool)
        .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a cron expression string into a [`Schedule`].
///
/// Expects a 6-field expression (seconds, minutes, hours, day-of-month, month,
/// day-of-week) or a 7-field expression (with year).
fn parse_cron(expr: &str) -> SchedulerResult<Schedule> {
    expr.parse::<Schedule>()
        .map_err(|e| SchedulerError::InvalidCron {
            expr: expr.to_string(),
            reason: e.to_string(),
        })
}

/// Derive a stable i64 advisory lock key from a job ID string.
///
/// Uses a simple FNV-1a hash to map arbitrary strings to i64.
fn advisory_lock_key(job_id: &str) -> i64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in job_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash as i64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_cron() {
        assert!(parse_cron("0 */15 * * * *").is_ok());
        assert!(parse_cron("0 0 2 * * *").is_ok());
    }

    #[test]
    fn test_parse_invalid_cron() {
        let err = parse_cron("not a cron").unwrap_err();
        assert!(matches!(err, SchedulerError::InvalidCron { .. }));
    }

    #[test]
    fn test_advisory_lock_key_deterministic() {
        let key1 = advisory_lock_key("crons:cleanup");
        let key2 = advisory_lock_key("crons:cleanup");
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_advisory_lock_key_different_for_different_ids() {
        let key1 = advisory_lock_key("crons:cleanup");
        let key2 = advisory_lock_key("crons:backup");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_scheduled_job_defaults() {
        let job = ScheduledJob {
            id: "test".to_string(),
            function_name: "test:func".to_string(),
            cron_expr: "0 * * * * *".to_string(),
            next_run_at: None,
            last_run_at: None,
            status: JobStatus::Idle,
            consecutive_failures: 0,
            max_retries: 3,
        };
        assert_eq!(job.status, JobStatus::Idle);
        assert_eq!(job.consecutive_failures, 0);
    }
}
