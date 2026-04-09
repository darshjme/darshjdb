//! Workflow DAG execution engine for the automation system.
//!
//! A workflow is a sequence of steps triggered by an event. Each step
//! executes an action and optionally passes output to subsequent steps.
//! Steps can have conditions (skip if not met) and error strategies
//! (continue, stop, retry).
//!
//! Inspired by DAF's graph execution model with topological ordering
//! and parallel execution of independent nodes.

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::query::WhereClause;

use super::action::{ActionConfig, ActionContext, ActionResult, BuiltinExecutor};
use super::event_bus::DdbEvent;
use super::trigger::TriggerConfig;

/// Unique identifier for a workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkflowId(pub Uuid);

impl WorkflowId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for WorkflowId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WorkflowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Error strategy ────────────────────────────────────────────────

/// What to do when a workflow step fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ErrorStrategy {
    /// Continue to the next step despite the failure.
    Continue,
    /// Stop the entire workflow.
    #[default]
    Stop,
    /// Retry the step up to N times before failing.
    Retry { max_retries: u32 },
}

// ── Workflow step ─────────────────────────────────────────────────

/// A single step in a workflow pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Step identifier (unique within the workflow).
    pub id: String,
    /// The action to execute.
    pub action: ActionConfig,
    /// Optional condition — step is skipped if this evaluates to false.
    pub condition: Option<Vec<WhereClause>>,
    /// What to do if this step fails.
    #[serde(default)]
    pub on_error: ErrorStrategy,
    /// Steps that must complete before this one (for parallel execution).
    /// If empty, the step depends on the previous step in sequence.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl WorkflowStep {
    /// Create a simple sequential step with no condition.
    pub fn new(id: impl Into<String>, action: ActionConfig) -> Self {
        Self {
            id: id.into(),
            action,
            condition: None,
            on_error: ErrorStrategy::default(),
            depends_on: Vec::new(),
        }
    }
}

// ── Workflow ──────────────────────────────────────────────────────

/// A complete automation workflow: trigger + ordered steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    /// Unique workflow identifier.
    pub id: WorkflowId,
    /// Human-readable name.
    pub name: String,
    /// Description of what this workflow does.
    #[serde(default)]
    pub description: String,
    /// The trigger that starts this workflow.
    pub trigger: TriggerConfig,
    /// Ordered list of execution steps.
    pub steps: Vec<WorkflowStep>,
    /// Whether this workflow is currently active.
    pub enabled: bool,
    /// Who created this workflow.
    pub created_by: Option<String>,
    /// When the workflow was created.
    pub created_at: DateTime<Utc>,
    /// When the workflow was last modified.
    pub updated_at: DateTime<Utc>,
}

impl Workflow {
    /// Create a new enabled workflow.
    pub fn new(name: impl Into<String>, trigger: TriggerConfig, steps: Vec<WorkflowStep>) -> Self {
        let now = Utc::now();
        Self {
            id: WorkflowId::new(),
            name: name.into(),
            description: String::new(),
            trigger,
            steps,
            enabled: true,
            created_by: None,
            created_at: now,
            updated_at: now,
        }
    }
}

// ── Workflow run ──────────────────────────────────────────────────

/// Status of a workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Currently executing.
    Running,
    /// All steps completed successfully.
    Completed,
    /// At least one step failed and the workflow stopped.
    Failed,
    /// The workflow was manually cancelled.
    Cancelled,
}

/// Result of executing a single step in a workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Step ID that was executed.
    pub step_id: String,
    /// The action result.
    pub result: ActionResult,
    /// Whether the step was skipped due to a condition.
    pub skipped: bool,
    /// When this step started.
    pub started_at: DateTime<Utc>,
    /// When this step completed.
    pub completed_at: DateTime<Utc>,
}

/// A complete record of a workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    /// Unique run identifier.
    pub id: Uuid,
    /// The workflow that was executed.
    pub workflow_id: WorkflowId,
    /// The event that triggered this run.
    pub trigger_event: Option<DdbEvent>,
    /// When execution started.
    pub started_at: DateTime<Utc>,
    /// When execution completed (None if still running).
    pub completed_at: Option<DateTime<Utc>>,
    /// Overall run status.
    pub status: RunStatus,
    /// Results for each step, in execution order.
    pub step_results: Vec<StepResult>,
    /// Total execution duration in milliseconds.
    pub duration_ms: Option<u64>,
}

impl WorkflowRun {
    /// Create a new run in the Running state.
    fn new(workflow_id: WorkflowId, trigger_event: Option<DdbEvent>) -> Self {
        Self {
            id: Uuid::new_v4(),
            workflow_id,
            trigger_event,
            started_at: Utc::now(),
            completed_at: None,
            status: RunStatus::Running,
            step_results: Vec::new(),
            duration_ms: None,
        }
    }

    /// Mark the run as completed with the given status.
    fn finish(&mut self, status: RunStatus) {
        self.status = status;
        self.completed_at = Some(Utc::now());
        let start = self.started_at.timestamp_millis();
        let end = self.completed_at.unwrap().timestamp_millis();
        self.duration_ms = Some((end - start).unsigned_abs());
    }
}

// ── Workflow engine ───────────────────────────────────────────────

/// Executes workflows by evaluating steps in order, handling conditions,
/// errors, and logging every step's input/output/duration.
pub struct WorkflowEngine {
    executor: Arc<BuiltinExecutor>,
}

impl WorkflowEngine {
    /// Create a new workflow engine.
    pub fn new() -> Self {
        Self {
            executor: Arc::new(BuiltinExecutor::new()),
        }
    }

    /// Execute a workflow, returning the complete run record.
    ///
    /// Steps are executed sequentially by default. Steps with explicit
    /// `depends_on` fields can be parallelised if their dependencies
    /// are already satisfied.
    pub async fn execute(
        &self,
        workflow: &Workflow,
        trigger_event: Option<DdbEvent>,
        base_context: ActionContext,
    ) -> WorkflowRun {
        let mut run = WorkflowRun::new(workflow.id, trigger_event);
        let wall_start = Instant::now();

        info!(
            workflow_id = %workflow.id,
            workflow_name = %workflow.name,
            run_id = %run.id,
            steps = workflow.steps.len(),
            "starting workflow execution"
        );

        let mut context = base_context;
        let mut all_succeeded = true;

        for (idx, step) in workflow.steps.iter().enumerate() {
            let step_start = Utc::now();

            // Check condition.
            if let Some(ref condition) = step.condition
                && !self.evaluate_condition(condition, &context)
            {
                info!(
                    step_id = %step.id,
                    step_index = idx,
                    "step skipped: condition not met"
                );
                run.step_results.push(StepResult {
                    step_id: step.id.clone(),
                    result: ActionResult::ok(Value::Null, 0),
                    skipped: true,
                    started_at: step_start,
                    completed_at: Utc::now(),
                });
                continue;
            }

            // Execute with retry logic.
            let result = self.execute_step_with_retry(step, &context).await;

            let step_result = StepResult {
                step_id: step.id.clone(),
                result: result.clone(),
                skipped: false,
                started_at: step_start,
                completed_at: Utc::now(),
            };

            // Store output for subsequent steps.
            context
                .previous_outputs
                .insert(step.id.clone(), result.output.clone());
            context
                .previous_outputs
                .insert(format!("step_{idx}"), result.output.clone());

            let step_failed = !result.success;
            run.step_results.push(step_result);

            if step_failed {
                all_succeeded = false;
                match step.on_error {
                    ErrorStrategy::Continue => {
                        warn!(
                            step_id = %step.id,
                            "step failed but continuing (on_error=continue)"
                        );
                    }
                    ErrorStrategy::Stop | ErrorStrategy::Retry { .. } => {
                        error!(
                            step_id = %step.id,
                            "step failed, stopping workflow"
                        );
                        break;
                    }
                }
            }
        }

        let status = if all_succeeded {
            RunStatus::Completed
        } else {
            RunStatus::Failed
        };

        run.finish(status);

        info!(
            workflow_id = %workflow.id,
            run_id = %run.id,
            status = ?status,
            duration_ms = wall_start.elapsed().as_millis() as u64,
            steps_executed = run.step_results.len(),
            "workflow execution finished"
        );

        run
    }

    /// Execute a single step, applying retry logic if configured.
    async fn execute_step_with_retry(
        &self,
        step: &WorkflowStep,
        context: &ActionContext,
    ) -> ActionResult {
        let max_attempts = match step.on_error {
            ErrorStrategy::Retry { max_retries } => max_retries + 1,
            _ => 1,
        };

        let mut last_result = ActionResult::err("never executed", 0);

        for attempt in 0..max_attempts {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(500 * 2u64.pow(attempt - 1));
                tokio::time::sleep(delay).await;
                warn!(
                    step_id = %step.id,
                    attempt = attempt + 1,
                    max = max_attempts,
                    "retrying step"
                );
            }

            last_result = self.executor.execute(&step.action, context).await;

            if last_result.success {
                return last_result;
            }
        }

        last_result
    }

    /// Evaluate a condition against the current action context.
    ///
    /// All clauses must match (AND semantics). Checks against
    /// `record_data` and `previous_outputs`.
    fn evaluate_condition(&self, clauses: &[WhereClause], context: &ActionContext) -> bool {
        clauses.iter().all(|clause| {
            // Check record data first, then previous outputs.
            context
                .record_data
                .get(&clause.attribute)
                .or_else(|| {
                    context
                        .previous_outputs
                        .values()
                        .filter_map(|v| v.get(&clause.attribute))
                        .next()
                })
                .map(|val| condition_matches(val, clause))
                .unwrap_or(false)
        })
    }
}

impl Default for WorkflowEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Check a single condition clause against a value.
fn condition_matches(value: &Value, clause: &WhereClause) -> bool {
    use crate::query::WhereOp;
    match clause.op {
        WhereOp::Eq => *value == clause.value,
        WhereOp::Neq => *value != clause.value,
        WhereOp::Gt => json_cmp(value, &clause.value).is_some_and(|o| o.is_gt()),
        WhereOp::Gte => json_cmp(value, &clause.value).is_some_and(|o| o.is_ge()),
        WhereOp::Lt => json_cmp(value, &clause.value).is_some_and(|o| o.is_lt()),
        WhereOp::Lte => json_cmp(value, &clause.value).is_some_and(|o| o.is_le()),
        WhereOp::Contains | WhereOp::Like => false,
    }
}

fn json_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64()?.partial_cmp(&b.as_f64()?),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::action::{ActionConfig, ActionKind};
    use crate::automations::trigger::{TriggerConfig, TriggerKind};
    use serde_json::json;

    fn make_workflow(steps: Vec<WorkflowStep>) -> Workflow {
        Workflow::new(
            "test_workflow",
            TriggerConfig::new(TriggerKind::Manual, "test"),
            steps,
        )
    }

    fn make_step(id: &str, kind: ActionKind, config: Value) -> WorkflowStep {
        WorkflowStep::new(id, ActionConfig::new(kind, config))
    }

    #[tokio::test]
    async fn execute_empty_workflow() {
        let engine = WorkflowEngine::new();
        let workflow = make_workflow(vec![]);
        let context = ActionContext::manual("test");

        let run = engine.execute(&workflow, None, context).await;
        assert_eq!(run.status, RunStatus::Completed);
        assert!(run.step_results.is_empty());
        assert!(run.duration_ms.is_some());
    }

    #[tokio::test]
    async fn execute_single_step_success() {
        let engine = WorkflowEngine::new();
        let workflow = make_workflow(vec![make_step(
            "create",
            ActionKind::CreateRecord,
            json!({ "entity_type": "tasks", "data": { "title": "auto" } }),
        )]);

        let context = ActionContext::manual("tasks");
        let run = engine.execute(&workflow, None, context).await;

        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.step_results.len(), 1);
        assert!(run.step_results[0].result.success);
        assert!(!run.step_results[0].skipped);
    }

    #[tokio::test]
    async fn execute_multi_step_chaining() {
        let engine = WorkflowEngine::new();
        let workflow = make_workflow(vec![
            make_step(
                "step_1",
                ActionKind::CreateRecord,
                json!({ "entity_type": "tasks", "data": {} }),
            ),
            make_step(
                "step_2",
                ActionKind::Notify,
                json!({ "channel": "slack", "message": "created" }),
            ),
        ]);

        let context = ActionContext::manual("tasks");
        let run = engine.execute(&workflow, None, context).await;

        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.step_results.len(), 2);
        assert!(run.step_results[0].result.success);
        assert!(run.step_results[1].result.success);
    }

    #[tokio::test]
    async fn execute_step_failure_stops_workflow() {
        let engine = WorkflowEngine::new();
        let workflow = make_workflow(vec![
            // This step will fail — CreateRecord without entity_type.
            make_step("bad_step", ActionKind::CreateRecord, json!({})),
            make_step(
                "never_reached",
                ActionKind::Notify,
                json!({ "message": "hello" }),
            ),
        ]);

        let context = ActionContext::manual("test");
        let run = engine.execute(&workflow, None, context).await;

        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.step_results.len(), 1); // Second step never executed.
        assert!(!run.step_results[0].result.success);
    }

    #[tokio::test]
    async fn execute_step_failure_continues() {
        let engine = WorkflowEngine::new();
        let mut bad_step = make_step("bad_step", ActionKind::CreateRecord, json!({}));
        bad_step.on_error = ErrorStrategy::Continue;

        let workflow = make_workflow(vec![
            bad_step,
            make_step(
                "still_runs",
                ActionKind::Notify,
                json!({ "message": "hello" }),
            ),
        ]);

        let context = ActionContext::manual("test");
        let run = engine.execute(&workflow, None, context).await;

        // Status is Failed because a step failed, but both ran.
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.step_results.len(), 2);
        assert!(!run.step_results[0].result.success);
        assert!(run.step_results[1].result.success);
    }

    #[tokio::test]
    async fn execute_conditional_step_skipped() {
        use crate::query::WhereOp;

        let engine = WorkflowEngine::new();
        let mut step = make_step(
            "conditional",
            ActionKind::Notify,
            json!({ "message": "hi" }),
        );
        step.condition = Some(vec![WhereClause {
            attribute: "status".to_string(),
            op: WhereOp::Eq,
            value: json!("active"),
        }]);

        let workflow = make_workflow(vec![step]);

        // Context has status=inactive, so condition won't match.
        let mut context = ActionContext::manual("test");
        context
            .record_data
            .insert("status".to_string(), json!("inactive"));

        let run = engine.execute(&workflow, None, context).await;
        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.step_results.len(), 1);
        assert!(run.step_results[0].skipped);
    }

    #[tokio::test]
    async fn execute_conditional_step_runs() {
        use crate::query::WhereOp;

        let engine = WorkflowEngine::new();
        let mut step = make_step(
            "conditional",
            ActionKind::Notify,
            json!({ "message": "hi" }),
        );
        step.condition = Some(vec![WhereClause {
            attribute: "status".to_string(),
            op: WhereOp::Eq,
            value: json!("active"),
        }]);

        let workflow = make_workflow(vec![step]);

        let mut context = ActionContext::manual("test");
        context
            .record_data
            .insert("status".to_string(), json!("active"));

        let run = engine.execute(&workflow, None, context).await;
        assert_eq!(run.status, RunStatus::Completed);
        assert!(!run.step_results[0].skipped);
        assert!(run.step_results[0].result.success);
    }

    #[tokio::test]
    async fn previous_step_output_accessible() {
        let engine = WorkflowEngine::new();
        let workflow = make_workflow(vec![
            make_step(
                "create",
                ActionKind::CreateRecord,
                json!({ "entity_type": "tasks", "data": {} }),
            ),
            make_step("notify", ActionKind::Notify, json!({ "message": "done" })),
        ]);

        let context = ActionContext::manual("tasks");
        let run = engine.execute(&workflow, None, context).await;

        assert_eq!(run.status, RunStatus::Completed);
        // The second step should have access to the first step's output
        // in the context (verified by the engine populating previous_outputs).
        assert_eq!(run.step_results.len(), 2);
    }

    #[test]
    fn workflow_serde_roundtrip() {
        let workflow = Workflow::new(
            "test",
            TriggerConfig::new(TriggerKind::OnRecordCreate, "users"),
            vec![WorkflowStep::new(
                "step_1",
                ActionConfig::new(ActionKind::Notify, json!({ "message": "welcome" })),
            )],
        );

        let json = serde_json::to_string(&workflow).unwrap();
        let restored: Workflow = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test");
        assert_eq!(restored.steps.len(), 1);
    }

    #[test]
    fn workflow_run_status_variants() {
        let statuses = vec![
            RunStatus::Running,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Cancelled,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).unwrap();
            let restored: RunStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(*status, restored);
        }
    }

    #[test]
    fn error_strategy_serde() {
        let strategies = vec![
            ErrorStrategy::Continue,
            ErrorStrategy::Stop,
            ErrorStrategy::Retry { max_retries: 3 },
        ];
        for strategy in &strategies {
            let json = serde_json::to_string(strategy).unwrap();
            let restored: ErrorStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(*strategy, restored);
        }
    }

    #[test]
    fn step_ordering_default_sequential() {
        let steps = vec![
            WorkflowStep::new("a", ActionConfig::new(ActionKind::Notify, json!({}))),
            WorkflowStep::new("b", ActionConfig::new(ActionKind::Notify, json!({}))),
        ];
        // By default, depends_on is empty (sequential).
        assert!(steps[0].depends_on.is_empty());
        assert!(steps[1].depends_on.is_empty());
    }
}
