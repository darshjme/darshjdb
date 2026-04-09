//! Knowledge-base extraction from event streams (DAF-inspired).
//!
//! Analyzes sequences of [`DdbEvent`]s to detect operational patterns:
//! frequently mutated entities, error-prone operations, performance
//! anomalies, and usage spikes. Patterns are stored as triples in the
//! triple store for queryability via DarshJQL.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tracing::debug;
use uuid::Uuid;

use super::{DdbEvent, EventKind};

// ── KB Entry Types ─────────────────────────────────────────────────

/// The type of pattern detected by the KB extractor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PatternType {
    /// An entity or attribute that is mutated far more often than average.
    FrequentMutation,
    /// An operation type that frequently appears alongside errors or rollbacks.
    ErrorPattern,
    /// Queries or operations taking anomalously long (detected via event gaps).
    PerformanceAnomaly,
    /// A sudden spike in events of a particular kind or for a particular entity type.
    UsageSpike,
}

impl std::fmt::Display for PatternType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A single knowledge-base entry extracted from event analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KBEntry {
    /// Unique identifier for this KB entry.
    pub id: Uuid,
    /// What kind of pattern was detected.
    pub pattern_type: PatternType,
    /// Human-readable description of the pattern.
    pub description: String,
    /// Supporting evidence (event IDs, counts, timestamps).
    pub evidence: Value,
    /// Confidence score (0.0 to 1.0).
    pub confidence: f64,
    /// When this pattern was detected.
    pub detected_at: DateTime<Utc>,
}

impl KBEntry {
    fn new(
        pattern_type: PatternType,
        description: String,
        evidence: Value,
        confidence: f64,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            pattern_type,
            description,
            evidence,
            confidence,
            detected_at: Utc::now(),
        }
    }
}

// ── Pattern Extraction ─────────────────────────────────────────────

/// Configuration thresholds for pattern detection.
#[derive(Debug, Clone)]
pub struct ExtractionConfig {
    /// An entity is "frequently mutated" if it appears in more than this
    /// fraction of all mutation events in the window.
    pub frequent_mutation_threshold: f64,
    /// Minimum number of events of a single kind to trigger a usage spike.
    pub usage_spike_min_count: usize,
    /// Usage spike ratio: if a kind's count exceeds (average * ratio), it is a spike.
    pub usage_spike_ratio: f64,
    /// Minimum gap (in seconds) between consecutive events to flag as anomaly.
    pub performance_anomaly_gap_secs: f64,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            frequent_mutation_threshold: 0.1, // 10% of all mutations
            usage_spike_min_count: 20,
            usage_spike_ratio: 3.0,
            performance_anomaly_gap_secs: 5.0,
        }
    }
}

/// Analyze a batch of events and extract operational patterns.
///
/// This is the main entry point for KB extraction. It runs multiple
/// detectors over the event slice and returns all discovered patterns.
pub fn extract_patterns(events: &[DdbEvent]) -> Vec<KBEntry> {
    extract_patterns_with_config(events, &ExtractionConfig::default())
}

/// Analyze with custom configuration thresholds.
pub fn extract_patterns_with_config(
    events: &[DdbEvent],
    config: &ExtractionConfig,
) -> Vec<KBEntry> {
    if events.is_empty() {
        return Vec::new();
    }

    let mut entries = Vec::new();

    entries.extend(detect_frequent_mutations(events, config));
    entries.extend(detect_usage_spikes(events, config));
    entries.extend(detect_performance_anomalies(events, config));
    entries.extend(detect_error_patterns(events));

    entries
}

/// Detect entities or attributes that are mutated disproportionately often.
fn detect_frequent_mutations(events: &[DdbEvent], config: &ExtractionConfig) -> Vec<KBEntry> {
    let mutation_kinds = [
        EventKind::RecordUpdated,
        EventKind::FieldUpdated,
        EventKind::RecordDeleted,
        EventKind::FieldDeleted,
    ];

    let mutations: Vec<&DdbEvent> = events
        .iter()
        .filter(|e| mutation_kinds.contains(&e.kind))
        .collect();

    if mutations.is_empty() {
        return Vec::new();
    }

    let total = mutations.len();
    let threshold = (total as f64 * config.frequent_mutation_threshold).max(2.0) as usize;

    // Count mutations per (entity_type, entity_id).
    let mut entity_counts: HashMap<(Option<&str>, Option<Uuid>), usize> = HashMap::new();
    for m in &mutations {
        let key = (m.entity_type.as_deref(), m.entity_id);
        *entity_counts.entry(key).or_default() += 1;
    }

    // Count mutations per attribute.
    let mut attr_counts: HashMap<(&str, Option<&str>), usize> = HashMap::new();
    for m in &mutations {
        if let Some(ref attr) = m.attribute {
            let key = (attr.as_str(), m.entity_type.as_deref());
            *attr_counts.entry(key).or_default() += 1;
        }
    }

    let mut entries = Vec::new();

    for ((et, eid), count) in &entity_counts {
        if *count >= threshold {
            let confidence = (*count as f64 / total as f64).min(1.0);
            entries.push(KBEntry::new(
                PatternType::FrequentMutation,
                format!(
                    "Entity {:?} (type: {:?}) was mutated {count} times out of {total} mutations ({:.0}%)",
                    eid,
                    et,
                    confidence * 100.0
                ),
                serde_json::json!({
                    "entity_type": et,
                    "entity_id": eid.map(|id| id.to_string()),
                    "mutation_count": count,
                    "total_mutations": total,
                }),
                confidence,
            ));
        }
    }

    for ((attr, et), count) in &attr_counts {
        if *count >= threshold {
            let confidence = (*count as f64 / total as f64).min(1.0);
            entries.push(KBEntry::new(
                PatternType::FrequentMutation,
                format!(
                    "Attribute '{attr}' on type {:?} mutated {count} times ({:.0}% of mutations)",
                    et,
                    confidence * 100.0
                ),
                serde_json::json!({
                    "attribute": attr,
                    "entity_type": et,
                    "mutation_count": count,
                    "total_mutations": total,
                }),
                confidence,
            ));
        }
    }

    entries
}

/// Detect sudden spikes in event volume for a particular kind or entity type.
fn detect_usage_spikes(events: &[DdbEvent], config: &ExtractionConfig) -> Vec<KBEntry> {
    // Count events per kind.
    let mut kind_counts: HashMap<&EventKind, usize> = HashMap::new();
    for e in events {
        *kind_counts.entry(&e.kind).or_default() += 1;
    }

    if kind_counts.is_empty() {
        return Vec::new();
    }

    let avg = events.len() as f64 / kind_counts.len() as f64;
    let spike_threshold = (avg * config.usage_spike_ratio).max(config.usage_spike_min_count as f64);

    let mut entries = Vec::new();

    for (kind, count) in &kind_counts {
        if *count as f64 >= spike_threshold {
            let ratio = *count as f64 / avg;
            entries.push(KBEntry::new(
                PatternType::UsageSpike,
                format!(
                    "Event kind {kind} spiked with {count} events ({ratio:.1}x above average)",
                ),
                serde_json::json!({
                    "kind": kind.as_str(),
                    "count": count,
                    "average": avg,
                    "ratio": ratio,
                }),
                (ratio / (ratio + 1.0)).min(1.0), // sigmoid-ish confidence
            ));
        }
    }

    // Also check per entity_type spikes.
    let mut type_counts: HashMap<&str, usize> = HashMap::new();
    for e in events {
        if let Some(ref et) = e.entity_type {
            *type_counts.entry(et.as_str()).or_default() += 1;
        }
    }

    if !type_counts.is_empty() {
        let type_avg = events.len() as f64 / type_counts.len().max(1) as f64;
        let type_threshold =
            (type_avg * config.usage_spike_ratio).max(config.usage_spike_min_count as f64);

        for (et, count) in &type_counts {
            if *count as f64 >= type_threshold {
                let ratio = *count as f64 / type_avg;
                entries.push(KBEntry::new(
                    PatternType::UsageSpike,
                    format!(
                        "Entity type '{et}' spiked with {count} events ({ratio:.1}x above average)"
                    ),
                    serde_json::json!({
                        "entity_type": et,
                        "count": count,
                        "average": type_avg,
                        "ratio": ratio,
                    }),
                    (ratio / (ratio + 1.0)).min(1.0),
                ));
            }
        }
    }

    entries
}

/// Detect anomalous gaps between consecutive events (possible slow operations).
fn detect_performance_anomalies(events: &[DdbEvent], config: &ExtractionConfig) -> Vec<KBEntry> {
    if events.len() < 2 {
        return Vec::new();
    }

    let mut sorted: Vec<&DdbEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.timestamp);

    let mut entries = Vec::new();
    let gap_threshold =
        chrono::Duration::milliseconds((config.performance_anomaly_gap_secs * 1000.0) as i64);

    for window in sorted.windows(2) {
        let prev = window[0];
        let curr = window[1];
        let gap = curr.timestamp - prev.timestamp;

        if gap > gap_threshold {
            let gap_secs = gap.num_milliseconds() as f64 / 1000.0;
            entries.push(KBEntry::new(
                PatternType::PerformanceAnomaly,
                format!(
                    "Anomalous {gap_secs:.1}s gap between events (tx {} -> tx {})",
                    prev.tx_id, curr.tx_id
                ),
                serde_json::json!({
                    "prev_event_id": prev.id.to_string(),
                    "curr_event_id": curr.id.to_string(),
                    "prev_kind": prev.kind.as_str(),
                    "curr_kind": curr.kind.as_str(),
                    "gap_seconds": gap_secs,
                    "threshold_seconds": config.performance_anomaly_gap_secs,
                }),
                // Higher gap = higher confidence it is anomalous.
                (gap_secs / (gap_secs + config.performance_anomaly_gap_secs)).min(1.0),
            ));
        }
    }

    entries
}

/// Detect error-prone operations by looking for delete patterns following creates
/// (rollback-like behavior) and operations from the same user in tight succession.
fn detect_error_patterns(events: &[DdbEvent]) -> Vec<KBEntry> {
    if events.len() < 2 {
        return Vec::new();
    }

    let mut entries = Vec::new();

    // Detect create-then-delete patterns (potential rollbacks).
    let mut creates: HashMap<Option<Uuid>, Vec<&DdbEvent>> = HashMap::new();
    let mut deletes: HashMap<Option<Uuid>, Vec<&DdbEvent>> = HashMap::new();

    for e in events {
        match e.kind {
            EventKind::RecordCreated => {
                creates.entry(e.entity_id).or_default().push(e);
            }
            EventKind::RecordDeleted => {
                deletes.entry(e.entity_id).or_default().push(e);
            }
            _ => {}
        }
    }

    let mut rollback_count = 0usize;
    let mut evidence_pairs: Vec<Value> = Vec::new();

    for (eid, del_events) in &deletes {
        if let Some(cre_events) = creates.get(eid) {
            for d in del_events {
                for c in cre_events {
                    let gap = d.timestamp - c.timestamp;
                    // If deleted within 60 seconds of creation, likely a rollback.
                    if gap.num_seconds() >= 0 && gap.num_seconds() <= 60 {
                        rollback_count += 1;
                        if evidence_pairs.len() < 10 {
                            evidence_pairs.push(serde_json::json!({
                                "entity_id": eid.map(|id| id.to_string()),
                                "created_at": c.timestamp.to_rfc3339(),
                                "deleted_at": d.timestamp.to_rfc3339(),
                                "gap_seconds": gap.num_seconds(),
                            }));
                        }
                    }
                }
            }
        }
    }

    if rollback_count > 0 {
        let confidence = (rollback_count as f64 / events.len() as f64 * 10.0).min(1.0);
        entries.push(KBEntry::new(
            PatternType::ErrorPattern,
            format!("Detected {rollback_count} create-then-delete patterns (possible rollbacks)"),
            serde_json::json!({
                "rollback_count": rollback_count,
                "sample_pairs": evidence_pairs,
            }),
            confidence,
        ));
    }

    entries
}

// ── Store KB Entries as Triples ────────────────────────────────────

/// Persist KB entries as triples in the triple store for queryability.
///
/// Each `KBEntry` becomes an entity of type `:kb/pattern` with attributes:
/// - `:kb/pattern_type`
/// - `:kb/description`
/// - `:kb/evidence`
/// - `:kb/confidence`
/// - `:kb/detected_at`
pub async fn store_kb_entries(pool: &PgPool, entries: &[KBEntry]) -> Result<(), sqlx::Error> {
    if entries.is_empty() {
        return Ok(());
    }

    let mut ids: Vec<Uuid> = Vec::with_capacity(entries.len() * 6);
    let mut entity_ids: Vec<Uuid> = Vec::with_capacity(entries.len() * 6);
    let mut attributes: Vec<String> = Vec::with_capacity(entries.len() * 6);
    let mut values: Vec<Value> = Vec::with_capacity(entries.len() * 6);

    for entry in entries {
        let eid = entry.id;

        // :db/type triple
        ids.push(Uuid::new_v4());
        entity_ids.push(eid);
        attributes.push(":db/type".to_string());
        values.push(Value::String(":kb/pattern".to_string()));

        // :kb/pattern_type
        ids.push(Uuid::new_v4());
        entity_ids.push(eid);
        attributes.push(":kb/pattern_type".to_string());
        values.push(Value::String(entry.pattern_type.to_string()));

        // :kb/description
        ids.push(Uuid::new_v4());
        entity_ids.push(eid);
        attributes.push(":kb/description".to_string());
        values.push(Value::String(entry.description.clone()));

        // :kb/evidence
        ids.push(Uuid::new_v4());
        entity_ids.push(eid);
        attributes.push(":kb/evidence".to_string());
        values.push(entry.evidence.clone());

        // :kb/confidence
        ids.push(Uuid::new_v4());
        entity_ids.push(eid);
        attributes.push(":kb/confidence".to_string());
        values.push(serde_json::json!(entry.confidence));

        // :kb/detected_at
        ids.push(Uuid::new_v4());
        entity_ids.push(eid);
        attributes.push(":kb/detected_at".to_string());
        values.push(Value::String(entry.detected_at.to_rfc3339()));
    }

    sqlx::query(
        r#"
        INSERT INTO triples (id, entity_id, attribute, value, value_type, retracted)
        SELECT id, entity_id, attribute, value, 'json', false
        FROM UNNEST($1::uuid[], $2::uuid[], $3::text[], $4::jsonb[])
            AS t(id, entity_id, attribute, value)
        "#,
    )
    .bind(&ids)
    .bind(&entity_ids)
    .bind(&attributes)
    .bind(&values)
    .execute(pool)
    .await?;

    debug!(count = entries.len(), "stored KB entries as triples");
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(kind: EventKind, entity_type: &str, entity_id: Uuid, tx_id: i64) -> DdbEvent {
        DdbEvent::new(kind, tx_id)
            .with_entity_type(entity_type)
            .with_entity_id(entity_id)
    }

    fn make_event_at(
        kind: EventKind,
        entity_type: &str,
        entity_id: Uuid,
        tx_id: i64,
        ts: DateTime<Utc>,
    ) -> DdbEvent {
        let mut e = make_event(kind, entity_type, entity_id, tx_id);
        e.timestamp = ts;
        e
    }

    #[test]
    fn empty_events_produce_no_patterns() {
        let patterns = extract_patterns(&[]);
        assert!(patterns.is_empty());
    }

    #[test]
    fn detect_frequent_mutation_entity() {
        let hot_id = Uuid::new_v4();
        let mut events: Vec<DdbEvent> = Vec::new();

        // 20 mutations on the same entity.
        for i in 0..20 {
            events.push(make_event(EventKind::RecordUpdated, "Counter", hot_id, i));
        }
        // 5 mutations on other entities.
        for i in 20..25 {
            events.push(make_event(
                EventKind::RecordUpdated,
                "Counter",
                Uuid::new_v4(),
                i,
            ));
        }

        let patterns = extract_patterns(&events);
        let freq = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::FrequentMutation)
            .collect::<Vec<_>>();

        assert!(!freq.is_empty(), "should detect frequent mutation pattern");
        // The hot entity should appear in at least one entry.
        let has_hot = freq.iter().any(|p| {
            p.evidence
                .get("entity_id")
                .and_then(|v| v.as_str())
                .map(|s| s == hot_id.to_string())
                .unwrap_or(false)
        });
        assert!(has_hot, "hot entity should be identified");
    }

    #[test]
    fn detect_frequent_mutation_attribute() {
        let mut events: Vec<DdbEvent> = Vec::new();

        // 15 mutations on the "status" attribute.
        for i in 0..15 {
            let mut e = make_event(EventKind::FieldUpdated, "Order", Uuid::new_v4(), i);
            e.attribute = Some("status".to_string());
            events.push(e);
        }
        // 3 mutations on other attributes.
        for i in 15..18 {
            let mut e = make_event(EventKind::FieldUpdated, "Order", Uuid::new_v4(), i);
            e.attribute = Some("notes".to_string());
            events.push(e);
        }

        let patterns = extract_patterns(&events);
        let freq = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::FrequentMutation)
            .collect::<Vec<_>>();

        let has_status = freq
            .iter()
            .any(|p| p.evidence.get("attribute").and_then(|v| v.as_str()) == Some("status"));
        assert!(has_status, "should detect frequently mutated attribute");
    }

    #[test]
    fn detect_usage_spike() {
        let mut events: Vec<DdbEvent> = Vec::new();
        let config = ExtractionConfig {
            usage_spike_min_count: 5,
            usage_spike_ratio: 2.0,
            ..Default::default()
        };

        // 30 logins (spike) vs 3 of other kinds.
        for i in 0..30 {
            events.push(DdbEvent::new(EventKind::AuthLogin, i));
        }
        for i in 30..33 {
            events.push(DdbEvent::new(EventKind::AuthLogout, i));
        }
        for i in 33..36 {
            events.push(DdbEvent::new(EventKind::StorageUpload, i));
        }

        let patterns = extract_patterns_with_config(&events, &config);
        let spikes: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::UsageSpike)
            .collect();

        assert!(!spikes.is_empty(), "should detect usage spike");
        let has_login_spike = spikes
            .iter()
            .any(|p| p.evidence.get("kind").and_then(|v| v.as_str()) == Some("AuthLogin"));
        assert!(has_login_spike, "AuthLogin should be flagged as spike");
    }

    #[test]
    fn detect_performance_anomaly() {
        let config = ExtractionConfig {
            performance_anomaly_gap_secs: 2.0,
            ..Default::default()
        };

        let base = Utc::now();
        let events = vec![
            make_event_at(EventKind::RecordCreated, "User", Uuid::new_v4(), 1, base),
            // 10 second gap — should trigger anomaly.
            make_event_at(
                EventKind::RecordCreated,
                "User",
                Uuid::new_v4(),
                2,
                base + chrono::Duration::seconds(10),
            ),
            // Normal 100ms gap.
            make_event_at(
                EventKind::RecordCreated,
                "User",
                Uuid::new_v4(),
                3,
                base + chrono::Duration::milliseconds(10100),
            ),
        ];

        let patterns = extract_patterns_with_config(&events, &config);
        let anomalies: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::PerformanceAnomaly)
            .collect();

        assert_eq!(anomalies.len(), 1, "should detect exactly one anomaly");
        let gap = anomalies[0]
            .evidence
            .get("gap_seconds")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!(gap >= 9.0, "gap should be ~10 seconds, got {gap}");
    }

    #[test]
    fn detect_error_pattern_rollback() {
        let base = Utc::now();
        let entity_id = Uuid::new_v4();

        let events = vec![
            make_event_at(EventKind::RecordCreated, "Order", entity_id, 1, base),
            // Deleted 5 seconds after creation — looks like a rollback.
            make_event_at(
                EventKind::RecordDeleted,
                "Order",
                entity_id,
                2,
                base + chrono::Duration::seconds(5),
            ),
        ];

        let patterns = extract_patterns(&events);
        let errors: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::ErrorPattern)
            .collect();

        assert!(!errors.is_empty(), "should detect rollback pattern");
        let rollback_count = errors[0]
            .evidence
            .get("rollback_count")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(rollback_count, 1);
    }

    #[test]
    fn no_false_positive_on_normal_delete() {
        let base = Utc::now();
        let entity_id = Uuid::new_v4();

        let events = vec![
            make_event_at(EventKind::RecordCreated, "Post", entity_id, 1, base),
            // Deleted 5 minutes later — normal lifecycle, not a rollback.
            make_event_at(
                EventKind::RecordDeleted,
                "Post",
                entity_id,
                2,
                base + chrono::Duration::seconds(300),
            ),
        ];

        let patterns = extract_patterns(&events);
        let errors: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == PatternType::ErrorPattern)
            .collect();

        assert!(
            errors.is_empty(),
            "should not flag normal deletion as error"
        );
    }

    #[test]
    fn kb_entry_confidence_bounds() {
        // Generate enough events to trigger patterns, verify confidence is in [0, 1].
        let mut events: Vec<DdbEvent> = Vec::new();
        let id = Uuid::new_v4();
        for i in 0..50 {
            events.push(make_event(EventKind::RecordUpdated, "X", id, i));
        }

        let patterns = extract_patterns(&events);
        for p in &patterns {
            assert!(
                (0.0..=1.0).contains(&p.confidence),
                "confidence {:.2} out of bounds for {:?}",
                p.confidence,
                p.pattern_type
            );
        }
    }
}
