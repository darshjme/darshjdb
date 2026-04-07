# Links, Lookups, and Rollups

DarshJDB's relational system is built on top of the EAV triple store's `ValueType::Reference` (type tag 5). It provides three primitives that together make DarshJDB a full relational BaaS:

- **Link** fields create bidirectional reference triples between entities.
- **Lookup** fields traverse a link and pull field values from linked records.
- **Rollup** fields traverse a link, collect target field values, and apply an aggregation function.

A **cascade** system handles cleanup when linked records are mutated or deleted, invalidating dependent lookups and rollups and emitting real-time change events.

## Link Fields

### Link Types

| Relationship | Key | Storage | Description |
|---|---|---|---|
| One-to-One | `one_to_one` | Direct reference triple | One source links to exactly one target. Adding a new link retracts the previous one. |
| One-to-Many | `one_to_many` | Direct reference triple | One source links to many targets. Multiple reference triples coexist on the source entity. |
| Many-to-Many | `many_to_many` | Junction entity | Many sources link to many targets. Uses junction entities (`link:{uuid}`) with `link/source` and `link/target` reference triples. |

### Storage Layout

**OneToOne / OneToMany**: Direct reference triples on the entity.

```
(source_id, "tasks", target_id, Reference)
```

With a symmetric backlink:

```
(target_id, "project", source_id, Reference)
```

**ManyToMany**: A junction entity is created for each link:

```
(junction_id, "db/type",       "link_junction", String)
(junction_id, "link/attribute", "tasks",         String)
(junction_id, "link/source",    source_id,       Reference)
(junction_id, "link/target",    target_id,       Reference)
```

A direct reference triple is also stored on the source for fast lookups.

### Symmetric / Bidirectional Links

When `symmetric` is true, adding a link from A to B automatically creates a reverse link from B to A using the `backlink_name` attribute. Removing the link retracts both directions.

Symmetric links require a `backlink_name` to be specified.

### Link Configuration

When creating a link field, you persist its metadata via `create_link`:

```json
{
  "source_table": "project",
  "target_table": "task",
  "relationship": "one_to_many",
  "symmetric": true,
  "backlink_name": "project"
}
```

The link metadata is stored on a sentinel entity `link_meta:{attribute}` with a deterministic UUID derived from the attribute name.

## API Endpoints

### Add a Link

```
POST /api/data/{entity}/{id}/link
```

```bash
curl -X POST http://localhost:4000/api/data/project/a1b2c3d4-.../link \
  -H "Content-Type: application/json" \
  -d '{
    "target_id": "e5f6g7h8-...",
    "attribute": "tasks",
    "relationship": "one_to_many",
    "symmetric": true,
    "backlink_name": "project"
  }'
```

Response:

```json
{
  "ok": true,
  "tx_id": 42
}
```

Default `relationship` is `one_to_many` if omitted. Default `symmetric` is `false`.

### Remove a Link

```
DELETE /api/data/{entity}/{id}/link
```

```bash
curl -X DELETE http://localhost:4000/api/data/project/a1b2c3d4-.../link \
  -H "Content-Type: application/json" \
  -d '{
    "target_id": "e5f6g7h8-...",
    "attribute": "tasks",
    "relationship": "one_to_many",
    "symmetric": true,
    "backlink_name": "project"
  }'
```

Response:

```json
{
  "ok": true,
  "message": "link removed"
}
```

For ManyToMany relationships, the junction entity is located and retracted along with the direct reference.

### Get Linked Entities

```
GET /api/data/{entity}/{id}/linked/{attribute}
```

```bash
curl http://localhost:4000/api/data/project/a1b2c3d4-.../linked/tasks
```

Response:

```json
{
  "entity_id": "a1b2c3d4-...",
  "attribute": "tasks",
  "linked_ids": [
    "e5f6g7h8-...",
    "i9j0k1l2-..."
  ],
  "count": 2
}
```

Returns UUIDs of all linked entities by following active, non-retracted reference triples.

## Lookup Fields

A lookup field follows a link to the target entity and reads a specific attribute value from it. For OneToOne links this produces a single value; for OneToMany/ManyToMany it produces an array.

### Lookup Resolution

The resolution process:
1. Follow the link field to find all linked entity IDs.
2. For each linked entity, read the specified lookup field attribute.
3. Collect all values into a flat array.

### Caching

Lookup results are cached with a configurable TTL (default 30 seconds). The cache is keyed by `(entity_id, link_field, lookup_field)`. Cache invalidation occurs when:

- The entity is updated (`invalidate_entity`)
- The link field changes (`invalidate_link`)
- The target entity's looked-up field changes (`invalidate_target_field`)

### Batch Resolution

For query result sets, `resolve_lookup_batch` efficiently resolves lookups for multiple entities in two SQL queries instead of N+1:

1. Batch-fetch all reference triples for the link attribute across all source entities.
2. Batch-fetch the lookup field values from all target entities.

### API Endpoint

```
GET /api/data/{entity}/{id}/lookup/{field}?link_field=...&lookup_field=...
```

```bash
curl "http://localhost:4000/api/data/task/a1b2c3d4-.../lookup/project_name?link_field=project&lookup_field=name"
```

Response:

```json
{
  "entity_id": "a1b2c3d4-...",
  "field": "name",
  "values": ["Dashboard Redesign"]
}
```

## Rollup Fields

A rollup field follows a link, collects a target attribute's values, and applies an aggregation function. Where possible, the aggregation is pushed down to SQL for efficiency; complex functions fall back to Rust-side computation.

### Rollup Functions

| Function | Key | SQL Pushable | Description |
|---|---|---|---|
| Count | `count` | Yes | Count of non-null linked values |
| Sum | `sum` | Yes | Numeric sum |
| Average | `average` | Yes | Arithmetic mean |
| Min | `min` | Yes | Minimum value |
| Max | `max` | Yes | Maximum value |
| Count All | `count_all` | Yes | Count of all linked records (regardless of field value) |
| Count Values | `count_values` | Yes | Count of linked records where the field has a value |
| Count Empty | `count_empty` | No | Count of linked records where the field has no value |
| Array Join | `array_join` | No | Join values with a separator string |
| Concatenate | `concatenate` | No | Concatenate values with no separator |

"SQL Pushable" means the aggregation runs inside PostgreSQL. Non-pushable functions fetch all values and compute in Rust.

### Empty Result Defaults

When no linked entities exist:

| Function | Empty Result |
|---|---|
| Count, CountAll, CountValues, CountEmpty | `0` |
| Sum | `0.0` |
| Average, Min, Max | `null` |
| ArrayJoin, Concatenate | `""` |

### API Endpoint

```
GET /api/data/{entity}/{id}/rollup/{field}?link_field=...&rollup_field=...&function=...
```

```bash
curl "http://localhost:4000/api/data/project/a1b2c3d4-.../rollup/total_hours?link_field=tasks&rollup_field=hours&function=sum"
```

Response:

```json
{
  "entity_id": "a1b2c3d4-...",
  "field": "hours",
  "function": "sum",
  "value": 127.5
}
```

For `array_join`, pass an optional `separator` parameter:

```bash
curl "http://localhost:4000/api/data/project/a1b2c3d4-.../rollup/tag_list?link_field=tasks&rollup_field=tag&function=array_join&separator=%20%7C%20"
```

Response:

```json
{
  "entity_id": "a1b2c3d4-...",
  "field": "tag",
  "function": "array_join",
  "value": "backend | frontend | design"
}
```

## Cascade Behavior

The cascade system handles relational side effects when entities change.

### On Entity Delete

When an entity is deleted, `cascade_delete` performs:

1. **Find reverse references**: Discovers all entities that link TO the deleted entity.
2. **Retract all links**: Retracts all reference triples involving the entity (both directions) and any junction entities.
3. **Invalidate caches**: Clears lookup/rollup caches for the deleted entity and all entities that referenced it.
4. **Emit change event**: Broadcasts a real-time `ChangeEvent` for all affected entity IDs.

### On Entity Update

When an entity's attribute changes, `cascade_update` performs:

1. **Find referencing entities**: Discovers entities that link to the changed entity.
2. **Invalidate lookups**: Clears lookup caches for the changed attributes and for all referencing entities.
3. **Emit change event**: Broadcasts changes for affected entities.

### On Link Change

When a link is added or removed, `cascade_link_change`:

1. **Invalidate both sides**: Clears caches for both source and target entities.
2. **Invalidate link**: Clears all lookup caches that traverse the changed link attribute.
3. **Emit change event**: Broadcasts for both source and target entity IDs.

### Cascade Event

Every cascade operation returns a `CascadeEvent`:

```json
{
  "trigger_entity_id": "a1b2c3d4-...",
  "operation": "delete",
  "affected_entity_ids": ["e5f6g7h8-...", "i9j0k1l2-..."],
  "invalidated_attributes": ["*"]
}
```

Operations: `delete`, `update`, `link_change`.

When `invalidated_attributes` contains `"*"`, all attributes on the affected entities should be considered stale.
