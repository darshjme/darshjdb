//! Solana-inspired parallel query execution for DarshanDB.
//!
//! Sealevel (Solana's runtime) executes non-conflicting transactions in parallel
//! by analyzing which accounts each transaction touches. This module applies the
//! same principle to DarshanQL batch operations: queries and mutations that touch
//! different entity types run concurrently, while conflicting operations are
//! serialized into sequential waves.
//!
//! # Conflict Model
//!
//! Two operations **conflict** if:
//! - They touch the same entity type, AND
//! - At least one of them is a mutation (write).
//!
//! Read-only queries on different entity types always run in parallel.
//! Read-only queries on the *same* entity type also run in parallel (readers
//! never conflict with readers).
//!
//! # Wave Scheduling
//!
//! Operations are grouped into waves using a greedy algorithm:
//! 1. For each operation, extract the set of entity types it touches.
//! 2. Walk the operation list in order. For each op, check if it conflicts
//!    with any op already in the current wave.
//! 3. If no conflict, add it to the current wave. If conflict, start a new wave.
//! 4. Execute each wave with `tokio::join_all` (parallel within wave).
//! 5. Waves execute sequentially to preserve causal ordering.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde_json::Value;

use crate::api::batch::BatchOp;

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Aggregated metrics for parallel batch execution.
#[derive(Debug, Default)]
pub struct ParallelMetrics {
    /// Total batches executed through the parallel executor.
    pub total_batches: AtomicU64,
    /// Total operations processed.
    pub total_ops: AtomicU64,
    /// Total operations that ran in parallel (wave size > 1).
    pub parallel_ops: AtomicU64,
    /// Total operations that ran sequentially (wave size == 1 or forced serial).
    pub sequential_ops: AtomicU64,
    /// Total waves executed across all batches.
    pub total_waves: AtomicU64,
    /// Cumulative batch duration in microseconds (for average calculation).
    pub cumulative_duration_us: AtomicU64,
    /// Sorted durations are tracked externally; these track p50/p95 approximation.
    /// We use a simple min/max/count for lightweight tracking.
    pub min_duration_us: AtomicU64,
    pub max_duration_us: AtomicU64,
}

impl ParallelMetrics {
    pub fn new() -> Self {
        Self {
            min_duration_us: AtomicU64::new(u64::MAX),
            max_duration_us: AtomicU64::new(0),
            ..Default::default()
        }
    }

    /// Record a completed batch execution.
    pub fn record_batch(&self, total_ops: u64, parallel_ops: u64, waves: u64, duration_us: u64) {
        self.total_batches.fetch_add(1, Ordering::Relaxed);
        self.total_ops.fetch_add(total_ops, Ordering::Relaxed);
        self.parallel_ops.fetch_add(parallel_ops, Ordering::Relaxed);
        self.sequential_ops
            .fetch_add(total_ops.saturating_sub(parallel_ops), Ordering::Relaxed);
        self.total_waves.fetch_add(waves, Ordering::Relaxed);
        self.cumulative_duration_us
            .fetch_add(duration_us, Ordering::Relaxed);

        // Update min.
        let mut current = self.min_duration_us.load(Ordering::Relaxed);
        while duration_us < current {
            match self.min_duration_us.compare_exchange_weak(
                current,
                duration_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }

        // Update max.
        let mut current = self.max_duration_us.load(Ordering::Relaxed);
        while duration_us > current {
            match self.max_duration_us.compare_exchange_weak(
                current,
                duration_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Snapshot current metrics for reporting.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let total = self.total_batches.load(Ordering::Relaxed);
        let cumulative = self.cumulative_duration_us.load(Ordering::Relaxed);
        let avg_us = if total > 0 { cumulative / total } else { 0 };

        MetricsSnapshot {
            total_batches: total,
            total_ops: self.total_ops.load(Ordering::Relaxed),
            parallel_ops: self.parallel_ops.load(Ordering::Relaxed),
            sequential_ops: self.sequential_ops.load(Ordering::Relaxed),
            total_waves: self.total_waves.load(Ordering::Relaxed),
            avg_batch_duration_us: avg_us,
            min_batch_duration_us: {
                let v = self.min_duration_us.load(Ordering::Relaxed);
                if v == u64::MAX { 0 } else { v }
            },
            max_batch_duration_us: self.max_duration_us.load(Ordering::Relaxed),
        }
    }
}

/// Point-in-time snapshot of parallel execution metrics.
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub total_batches: u64,
    pub total_ops: u64,
    pub parallel_ops: u64,
    pub sequential_ops: u64,
    pub total_waves: u64,
    pub avg_batch_duration_us: u64,
    pub min_batch_duration_us: u64,
    pub max_batch_duration_us: u64,
}

// ---------------------------------------------------------------------------
// Entity type extraction
// ---------------------------------------------------------------------------

/// Categorize an operation as read-only or mutating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Read,
    Write,
}

/// Metadata extracted from a batch operation for conflict analysis.
#[derive(Debug, Clone)]
pub struct OpProfile {
    /// Original index in the batch (for result ordering).
    pub index: usize,
    /// Entity types this operation touches.
    pub entity_types: HashSet<String>,
    /// Whether this operation reads or writes.
    pub kind: OpKind,
}

/// Extract the entity types a batch operation touches, plus its read/write kind.
pub fn profile_op(index: usize, op: &BatchOp) -> OpProfile {
    match op {
        BatchOp::Query { body, .. } => {
            let types = extract_entity_types_from_query(body);
            OpProfile {
                index,
                entity_types: types,
                kind: OpKind::Read,
            }
        }
        BatchOp::Mutate { body, .. } => {
            let types = extract_entity_types_from_mutate(body);
            OpProfile {
                index,
                entity_types: types,
                kind: OpKind::Write,
            }
        }
        BatchOp::Fn { name, .. } => {
            // Functions are opaque -- we cannot know what they touch.
            // Treat them as writes on a synthetic entity type derived from
            // the function name to prevent parallel execution with anything
            // that might conflict. Two different functions can still run
            // in parallel with each other.
            let mut types = HashSet::new();
            types.insert(format!("__fn:{name}"));
            OpProfile {
                index,
                entity_types: types,
                kind: OpKind::Write,
            }
        }
    }
}

/// Extract entity types from a DarshanQL query body.
///
/// DarshanQL queries use a `"type"` field to specify the entity type.
/// Older-style queries use top-level keys as entity type names.
fn extract_entity_types_from_query(body: &Value) -> HashSet<String> {
    let mut types = HashSet::new();

    if let Some(obj) = body.as_object() {
        // New-style: { "type": "User", "$where": ... }
        if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
            types.insert(t.to_string());
            return types;
        }

        // Old-style: { "users": { "$where": ... } }
        // Top-level keys that don't start with '$' are entity types.
        for key in obj.keys() {
            if !key.starts_with('$') {
                types.insert(key.clone());
            }
        }
    }

    // If we couldn't determine the entity type, use a wildcard that
    // conflicts with everything.
    if types.is_empty() {
        types.insert("__unknown".to_string());
    }

    types
}

/// Extract entity types from a mutation body.
///
/// Mutations contain a `"mutations"` array where each element has an
/// `"entity"` field specifying the target entity type.
fn extract_entity_types_from_mutate(body: &Value) -> HashSet<String> {
    let mut types = HashSet::new();

    if let Some(mutations) = body.get("mutations").and_then(|m| m.as_array()) {
        for mutation in mutations {
            if let Some(entity) = mutation.get("entity").and_then(|e| e.as_str()) {
                types.insert(entity.to_string());
            }
        }
    }

    if types.is_empty() {
        types.insert("__unknown".to_string());
    }

    types
}

// ---------------------------------------------------------------------------
// Conflict detection & wave scheduling
// ---------------------------------------------------------------------------

/// Two operations conflict if they touch overlapping entity types and at
/// least one is a write.
fn ops_conflict(a: &OpProfile, b: &OpProfile) -> bool {
    // Two reads never conflict.
    if a.kind == OpKind::Read && b.kind == OpKind::Read {
        return false;
    }

    // Check for entity type overlap.
    // The __unknown wildcard conflicts with everything.
    if a.entity_types.contains("__unknown") || b.entity_types.contains("__unknown") {
        return true;
    }

    a.entity_types
        .intersection(&b.entity_types)
        .next()
        .is_some()
}

/// A wave is a group of non-conflicting operations that can execute in parallel.
#[derive(Debug)]
pub struct Wave {
    /// Indices into the original operation list.
    pub op_indices: Vec<usize>,
}

/// Schedule operations into waves of non-conflicting groups.
///
/// Uses a greedy first-fit algorithm: for each operation, try to place it in
/// the earliest wave where it doesn't conflict with any existing member.
/// This preserves causal ordering (an op never runs before an op that was
/// listed earlier and conflicts with it).
pub fn schedule_waves(profiles: &[OpProfile]) -> Vec<Wave> {
    if profiles.is_empty() {
        return Vec::new();
    }

    // Track which profiles are in each wave for conflict checking.
    let mut waves: Vec<(Wave, Vec<usize>)> = Vec::new(); // (wave, profile indices in wave)

    for (i, profile) in profiles.iter().enumerate() {
        let mut placed = false;

        for (wave, members) in waves.iter_mut() {
            // Check if this profile conflicts with any member of this wave.
            let conflicts = members
                .iter()
                .any(|&member_idx| ops_conflict(profile, &profiles[member_idx]));

            if !conflicts {
                wave.op_indices.push(profile.index);
                members.push(i);
                placed = true;
                break;
            }
        }

        if !placed {
            waves.push((
                Wave {
                    op_indices: vec![profile.index],
                },
                vec![i],
            ));
        }
    }

    waves.into_iter().map(|(wave, _)| wave).collect()
}

/// Compute scheduling statistics for logging.
pub struct ScheduleStats {
    pub total_ops: usize,
    pub wave_count: usize,
    pub parallel_ops: usize,
    /// Map from wave index to its size.
    pub wave_sizes: Vec<usize>,
}

pub fn compute_stats(waves: &[Wave], total_ops: usize) -> ScheduleStats {
    let wave_sizes: Vec<usize> = waves.iter().map(|w| w.op_indices.len()).collect();
    let parallel_ops = wave_sizes.iter().filter(|&&s| s > 1).copied().sum();

    ScheduleStats {
        total_ops,
        wave_count: waves.len(),
        parallel_ops,
        wave_sizes,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_query_op(id: &str, body: Value) -> BatchOp {
        BatchOp::Query {
            id: id.to_string(),
            body,
        }
    }

    fn make_mutate_op(id: &str, entity: &str) -> BatchOp {
        BatchOp::Mutate {
            id: id.to_string(),
            body: json!({
                "mutations": [{ "op": "set", "entity": entity, "data": { "x": 1 } }]
            }),
        }
    }

    fn make_fn_op(id: &str, name: &str) -> BatchOp {
        BatchOp::Fn {
            id: id.to_string(),
            name: name.to_string(),
            args: Value::Null,
        }
    }

    // -- Entity type extraction --

    #[test]
    fn extract_entity_types_query_new_style() {
        let body = json!({"type": "User", "$where": []});
        let types = extract_entity_types_from_query(&body);
        assert_eq!(types.len(), 1);
        assert!(types.contains("User"));
    }

    #[test]
    fn extract_entity_types_query_old_style() {
        let body = json!({"users": {"$where": {"active": true}}, "posts": {}});
        let types = extract_entity_types_from_query(&body);
        assert_eq!(types.len(), 2);
        assert!(types.contains("users"));
        assert!(types.contains("posts"));
    }

    #[test]
    fn extract_entity_types_query_unknown_fallback() {
        let body = json!("not an object");
        let types = extract_entity_types_from_query(&body);
        assert!(types.contains("__unknown"));
    }

    #[test]
    fn extract_entity_types_mutate() {
        let body = json!({
            "mutations": [
                { "op": "set", "entity": "User", "data": {} },
                { "op": "set", "entity": "Post", "data": {} }
            ]
        });
        let types = extract_entity_types_from_mutate(&body);
        assert_eq!(types.len(), 2);
        assert!(types.contains("User"));
        assert!(types.contains("Post"));
    }

    #[test]
    fn extract_entity_types_mutate_empty() {
        let body = json!({});
        let types = extract_entity_types_from_mutate(&body);
        assert!(types.contains("__unknown"));
    }

    // -- Conflict detection --

    #[test]
    fn two_reads_same_entity_no_conflict() {
        let a = OpProfile {
            index: 0,
            entity_types: HashSet::from(["User".into()]),
            kind: OpKind::Read,
        };
        let b = OpProfile {
            index: 1,
            entity_types: HashSet::from(["User".into()]),
            kind: OpKind::Read,
        };
        assert!(!ops_conflict(&a, &b));
    }

    #[test]
    fn read_write_same_entity_conflict() {
        let a = OpProfile {
            index: 0,
            entity_types: HashSet::from(["User".into()]),
            kind: OpKind::Read,
        };
        let b = OpProfile {
            index: 1,
            entity_types: HashSet::from(["User".into()]),
            kind: OpKind::Write,
        };
        assert!(ops_conflict(&a, &b));
    }

    #[test]
    fn write_write_same_entity_conflict() {
        let a = OpProfile {
            index: 0,
            entity_types: HashSet::from(["User".into()]),
            kind: OpKind::Write,
        };
        let b = OpProfile {
            index: 1,
            entity_types: HashSet::from(["User".into()]),
            kind: OpKind::Write,
        };
        assert!(ops_conflict(&a, &b));
    }

    #[test]
    fn read_write_different_entities_no_conflict() {
        let a = OpProfile {
            index: 0,
            entity_types: HashSet::from(["User".into()]),
            kind: OpKind::Read,
        };
        let b = OpProfile {
            index: 1,
            entity_types: HashSet::from(["Post".into()]),
            kind: OpKind::Write,
        };
        assert!(!ops_conflict(&a, &b));
    }

    #[test]
    fn unknown_entity_always_conflicts_with_write() {
        let a = OpProfile {
            index: 0,
            entity_types: HashSet::from(["__unknown".into()]),
            kind: OpKind::Read,
        };
        let b = OpProfile {
            index: 1,
            entity_types: HashSet::from(["User".into()]),
            kind: OpKind::Write,
        };
        assert!(ops_conflict(&a, &b));
    }

    #[test]
    fn unknown_entities_two_reads_no_conflict() {
        let a = OpProfile {
            index: 0,
            entity_types: HashSet::from(["__unknown".into()]),
            kind: OpKind::Read,
        };
        let b = OpProfile {
            index: 1,
            entity_types: HashSet::from(["__unknown".into()]),
            kind: OpKind::Read,
        };
        assert!(!ops_conflict(&a, &b));
    }

    // -- Wave scheduling --

    #[test]
    fn schedule_all_reads_single_wave() {
        let ops = [
            make_query_op("q1", json!({"type": "User"})),
            make_query_op("q2", json!({"type": "Post"})),
            make_query_op("q3", json!({"type": "User"})),
        ];
        let profiles: Vec<_> = ops
            .iter()
            .enumerate()
            .map(|(i, op)| profile_op(i, op))
            .collect();
        let waves = schedule_waves(&profiles);

        // All reads, no conflicts -> single wave.
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].op_indices.len(), 3);
    }

    #[test]
    fn schedule_read_write_different_entities_single_wave() {
        let ops = [
            make_query_op("q1", json!({"type": "User"})),
            make_mutate_op("m1", "Post"),
        ];
        let profiles: Vec<_> = ops
            .iter()
            .enumerate()
            .map(|(i, op)| profile_op(i, op))
            .collect();
        let waves = schedule_waves(&profiles);

        // Different entities -> single wave.
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].op_indices.len(), 2);
    }

    #[test]
    fn schedule_read_write_same_entity_two_waves() {
        let ops = [
            make_query_op("q1", json!({"type": "User"})),
            make_mutate_op("m1", "User"),
        ];
        let profiles: Vec<_> = ops
            .iter()
            .enumerate()
            .map(|(i, op)| profile_op(i, op))
            .collect();
        let waves = schedule_waves(&profiles);

        // Same entity, one write -> two waves.
        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0].op_indices, vec![0]);
        assert_eq!(waves[1].op_indices, vec![1]);
    }

    #[test]
    fn schedule_complex_mixed_batch() {
        // q1: read User     (idx=0)
        // q2: read Post     (idx=1)
        // m1: write User    (idx=2)
        // q3: read Comment  (idx=3)
        // m2: write Post    (idx=4)
        // q4: read User     (idx=5)
        let ops = [
            make_query_op("q1", json!({"type": "User"})),
            make_query_op("q2", json!({"type": "Post"})),
            make_mutate_op("m1", "User"),
            make_query_op("q3", json!({"type": "Comment"})),
            make_mutate_op("m2", "Post"),
            make_query_op("q4", json!({"type": "User"})),
        ];
        let profiles: Vec<_> = ops
            .iter()
            .enumerate()
            .map(|(i, op)| profile_op(i, op))
            .collect();
        let waves = schedule_waves(&profiles);

        // Greedy first-fit scheduling:
        // q1(User-R)    -> wave 0 (empty)
        // q2(Post-R)    -> wave 0 (reads don't conflict)
        // m1(User-W)    -> wave 0 conflict (q1 User read+write) -> wave 1
        // q3(Comment-R) -> wave 0 (no overlap, all reads) -> wave 0
        // m2(Post-W)    -> wave 0 conflict (q2 Post read+write) -> wave 1 (no overlap with m1 User-W)
        // q4(User-R)    -> wave 0 (reads don't conflict with reads) -> wave 0
        //
        // Wave 0: [q1, q2, q3, q4] (all reads, fully parallel)
        // Wave 1: [m1, m2]         (writes on different entities, parallel)

        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0].op_indices, vec![0, 1, 3, 5]); // q1, q2, q3, q4
        assert_eq!(waves[1].op_indices, vec![2, 4]); // m1, m2
    }

    #[test]
    fn schedule_function_ops() {
        let ops = [
            make_fn_op("f1", "compute_stats"),
            make_fn_op("f2", "send_email"),
            make_fn_op("f3", "compute_stats"),
        ];
        let profiles: Vec<_> = ops
            .iter()
            .enumerate()
            .map(|(i, op)| profile_op(i, op))
            .collect();
        let waves = schedule_waves(&profiles);

        // f1 and f2 are different functions -> parallel.
        // f3 conflicts with f1 (same function name) -> new wave.
        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0].op_indices.len(), 2); // f1, f2
        assert_eq!(waves[1].op_indices.len(), 1); // f3
    }

    #[test]
    fn schedule_empty_batch() {
        let waves = schedule_waves(&[]);
        assert!(waves.is_empty());
    }

    #[test]
    fn schedule_single_op() {
        let ops = [make_query_op("q1", json!({"type": "User"}))];
        let profiles: Vec<_> = ops
            .iter()
            .enumerate()
            .map(|(i, op)| profile_op(i, op))
            .collect();
        let waves = schedule_waves(&profiles);

        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].op_indices.len(), 1);
    }

    // -- Profile extraction --

    #[test]
    fn profile_query_op() {
        let op = make_query_op("q1", json!({"type": "User"}));
        let profile = profile_op(0, &op);
        assert_eq!(profile.kind, OpKind::Read);
        assert!(profile.entity_types.contains("User"));
    }

    #[test]
    fn profile_mutate_op() {
        let op = make_mutate_op("m1", "Post");
        let profile = profile_op(0, &op);
        assert_eq!(profile.kind, OpKind::Write);
        assert!(profile.entity_types.contains("Post"));
    }

    #[test]
    fn profile_fn_op() {
        let op = make_fn_op("f1", "hello");
        let profile = profile_op(0, &op);
        assert_eq!(profile.kind, OpKind::Write);
        assert!(profile.entity_types.contains("__fn:hello"));
    }

    // -- Metrics --

    #[test]
    fn metrics_record_and_snapshot() {
        let metrics = ParallelMetrics::new();
        metrics.record_batch(10, 8, 3, 5000);
        metrics.record_batch(5, 2, 2, 3000);

        let snap = metrics.snapshot();
        assert_eq!(snap.total_batches, 2);
        assert_eq!(snap.total_ops, 15);
        assert_eq!(snap.parallel_ops, 10);
        assert_eq!(snap.sequential_ops, 5);
        assert_eq!(snap.total_waves, 5);
        assert_eq!(snap.avg_batch_duration_us, 4000);
        assert_eq!(snap.min_batch_duration_us, 3000);
        assert_eq!(snap.max_batch_duration_us, 5000);
    }

    // -- Stats --

    #[test]
    fn compute_stats_mixed() {
        let waves = vec![
            Wave {
                op_indices: vec![0, 1, 2],
            },
            Wave {
                op_indices: vec![3],
            },
            Wave {
                op_indices: vec![4, 5],
            },
        ];
        let stats = compute_stats(&waves, 6);
        assert_eq!(stats.total_ops, 6);
        assert_eq!(stats.wave_count, 3);
        assert_eq!(stats.parallel_ops, 5); // 3 + 2 from waves with size > 1
        assert_eq!(stats.wave_sizes, vec![3, 1, 2]);
    }
}
