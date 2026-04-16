# Event Bus

DarshJDB's event bus captures all mutations, auth events, storage operations, and custom events into a unified stream. Events are broadcast to filtered subscribers in real time and persisted to a `ddb_events` table for audit trails and knowledge-base extraction.

## Architecture

```
Mutation ----publish----> EventBus ---broadcast---> Subscriber A (filtered)
                            |                  ---> Subscriber B (filtered)
                            v
                       EventLogger ---batch---> ddb_events table
                            |
                            v
                       KB Extractor ---> triples (KBEntry patterns)
```

The bus uses `tokio::sync::broadcast` internally. Subscribers receive cloned events and apply their own filters client-side within `EventStream`.

## EventKind

Every event has a `kind` field identifying what happened. There are 17 built-in kinds plus a `Custom(String)` variant for application-defined events.

| Kind                  | Description                                    |
|-----------------------|------------------------------------------------|
| `RecordCreated`       | New record inserted                            |
| `RecordUpdated`       | Existing record modified                       |
| `RecordDeleted`       | Record removed                                 |
| `RecordBulkCreated`   | Batch insert operation                         |
| `FieldCreated`        | New field/attribute added to a record          |
| `FieldUpdated`        | Existing field value changed                   |
| `FieldDeleted`        | Field removed from a record                   |
| `ViewCreated`         | New view defined                               |
| `ViewUpdated`         | View definition modified                       |
| `ViewDeleted`         | View removed                                   |
| `AuthLogin`           | User signed in                                 |
| `AuthLogout`          | User signed out                                |
| `AuthSignup`          | New user registered                            |
| `StorageUpload`       | File uploaded to storage                       |
| `StorageDelete`       | File deleted from storage                      |
| `FunctionExecuted`    | Server-side function invoked                   |
| `AutomationTriggered` | Automation rule fired                          |
| `Custom(name)`        | Application-defined event                      |

Each kind serializes to a stable string (e.g., `"RecordCreated"`, `"custom:webhook.fired"`) for database storage and round-trips without loss.

## DdbEvent Structure

```rust
pub struct DdbEvent {
    pub id: Uuid,                         // Unique event ID
    pub kind: EventKind,                  // What happened
    pub entity_type: Option<String>,      // e.g. "User", "Post"
    pub entity_id: Option<Uuid>,          // Specific entity affected
    pub attribute: Option<String>,        // For field-level events
    pub old_value: Option<Value>,         // Previous value
    pub new_value: Option<Value>,         // New value
    pub user_id: Option<Uuid>,           // Who triggered it
    pub timestamp: DateTime<Utc>,         // When
    pub metadata: HashMap<String, Value>, // Arbitrary key-value pairs
    pub tx_id: i64,                       // Transaction ID
}
```

### Builder Pattern

Events are constructed with a fluent builder:

```rust
let event = DdbEvent::new(EventKind::RecordUpdated, tx_id)
    .with_entity_type("Post")
    .with_entity_id(post_id)
    .with_attribute("title")
    .with_values(
        Some(json!("Old Title")),
        Some(json!("New Title")),
    )
    .with_user(user_id)
    .with_metadata("source", json!("api"));
```

## EventBus

### Creating the Bus

```rust
// With persistence (production)
let bus = EventBus::new(pg_pool, 1024); // capacity = broadcast channel size

// Without persistence (testing)
let bus = EventBus::new_without_logger(64);
```

### Publishing

```rust
bus.publish(event);
```

Events are sent to both:
1. The persistence logger (non-blocking, drops on full channel)
2. All live broadcast subscribers

### Subscribing

```rust
let mut stream = bus.subscribe(EventFilter::all());

// In an async loop:
while let Some(event) = stream.recv().await {
    println!("{}: {:?}", event.kind, event.entity_type);
}
```

### Subscriber Count

```rust
let count = bus.subscriber_count();
```

## EventFilter

Filters are applied client-side within each `EventStream`. A filter with all fields set to `None` matches everything.

```rust
// All events
let filter = EventFilter::all();

// Only record mutations
let filter = EventFilter::all()
    .with_kinds(vec![EventKind::RecordCreated, EventKind::RecordUpdated]);

// Only User entity events
let filter = EventFilter::all()
    .with_entity_types(vec!["User".to_string()]);

// Specific entity by ID
let filter = EventFilter::all()
    .with_entity_ids(vec![entity_uuid]);

// Combined (AND logic)
let filter = EventFilter::all()
    .with_kinds(vec![EventKind::RecordCreated])
    .with_entity_types(vec!["User".to_string()])
    .with_entity_ids(vec![entity_uuid]);
```

Filter criteria are ANDed: an event must match all specified criteria to pass through.

## EventLogger (Batch Persistence)

The `EventLogger` runs as a background tokio task and writes events to the `ddb_events` table in batches, amortizing Postgres round-trip costs.

### Flush Strategy

- **Time-based**: flushes every 100ms
- **Count-based**: flushes when 100 events accumulate
- Whichever threshold is reached first triggers the flush

### Batch INSERT

Events are inserted using `UNNEST` arrays for maximum throughput:

```sql
INSERT INTO ddb_events (id, kind, entity_type, entity_id, attribute,
                        old_value, new_value, user_id, timestamp, metadata, tx_id)
SELECT * FROM UNNEST($1::uuid[], $2::text[], $3::text[], $4::uuid[], $5::text[],
                     $6::jsonb[], $7::jsonb[], $8::uuid[], $9::timestamptz[], $10::jsonb[], $11::bigint[])
```

### ddb_events Table Schema

```sql
CREATE TABLE ddb_events (
    id          UUID PRIMARY KEY,
    kind        TEXT NOT NULL,
    entity_type TEXT,
    entity_id   UUID,
    attribute   TEXT,
    old_value   JSONB,
    new_value   JSONB,
    user_id     UUID,
    timestamp   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    metadata    JSONB NOT NULL DEFAULT '{}',
    tx_id       BIGINT NOT NULL
);
```

Indexed on: `kind`, `entity_type`, `entity_id`, `timestamp`, `tx_id`.

## KB Extraction

The knowledge-base extractor analyzes batches of events to detect four types of operational patterns.

### Pattern Types

| Pattern               | Description                                              |
|-----------------------|----------------------------------------------------------|
| `FrequentMutation`    | Entity or attribute mutated disproportionately often     |
| `ErrorPattern`        | Create-then-delete sequences (possible rollbacks)        |
| `PerformanceAnomaly`  | Anomalous time gaps between consecutive events           |
| `UsageSpike`          | Sudden surge in events of a particular kind/entity type  |

### Usage

```rust
use darshjdb::events::kb::{extract_patterns, extract_patterns_with_config, ExtractionConfig};

// Default thresholds
let patterns = extract_patterns(&events);

// Custom thresholds
let config = ExtractionConfig {
    frequent_mutation_threshold: 0.1,   // 10% of mutations
    usage_spike_min_count: 20,
    usage_spike_ratio: 3.0,
    performance_anomaly_gap_secs: 5.0,
};
let patterns = extract_patterns_with_config(&events, &config);
```

### KBEntry Structure

Each detected pattern becomes a `KBEntry` with:

- `pattern_type` -- one of the four types above
- `description` -- human-readable explanation
- `evidence` -- JSON with supporting data (counts, IDs, timestamps)
- `confidence` -- score from 0.0 to 1.0
- `detected_at` -- when the pattern was identified

### Persistence as Triples

KB entries are stored as triples in the triple store under the `:kb/pattern` entity type:

```
:db/type          -> ":kb/pattern"
:kb/pattern_type  -> "FrequentMutation"
:kb/description   -> "Entity ... was mutated 20 times ..."
:kb/evidence      -> { ... }
:kb/confidence    -> 0.8
:kb/detected_at   -> "2026-04-07T12:00:00Z"
```

This makes patterns queryable via DarshJQL alongside application data.
