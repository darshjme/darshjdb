# History & Snapshots

DarshJDB provides full version history for every record and table-level snapshots, built on the immutable EAV triple store. Every mutation creates a new transaction (`tx_id`), and the complete state of any record can be reconstructed at any version or point in time by replaying triples in order.

## How Version History Works

Each triple carries a `tx_id` (monotonically increasing transaction counter) and a `retracted` flag. To update a field, the old triple is retracted and a new one is asserted in the same transaction. Deleting a record retracts all its triples.

Version reconstruction replays all triples for an entity in `tx_id` order, grouping them by transaction. Each transaction boundary produces a version with:

- **Version number**: 1-based, chronological
- **tx_id**: The transaction that created this version
- **changed_by**: Extracted from the `_changed_by` meta-attribute (if present)
- **changed_at**: Timestamp of the transaction
- **changes**: List of field-level diffs (added, modified, or removed)
- **snapshot**: The complete record state after this version

Change types: `added` (new attribute), `modified` (value changed), `removed` (attribute retracted).

## Viewing Record History

```bash
curl http://localhost:3000/api/records/{entity_id}/history?limit=10 \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "versions": [
    {
      "version_number": 1,
      "tx_id": 42,
      "changed_by": "user-uuid",
      "changed_at": "2026-04-01T10:00:00Z",
      "changes": [
        {
          "attribute": "user/email",
          "old_value": null,
          "new_value": "alice@example.com",
          "change_type": "added"
        },
        {
          "attribute": "user/name",
          "old_value": null,
          "new_value": "Alice",
          "change_type": "added"
        }
      ],
      "snapshot": {
        "user/name": "Alice",
        "user/email": "alice@example.com"
      }
    },
    {
      "version_number": 2,
      "tx_id": 57,
      "changed_by": "user-uuid",
      "changed_at": "2026-04-05T14:30:00Z",
      "changes": [
        {
          "attribute": "user/email",
          "old_value": "alice@example.com",
          "new_value": "alice@newdomain.com",
          "change_type": "modified"
        }
      ],
      "snapshot": {
        "user/name": "Alice",
        "user/email": "alice@newdomain.com"
      }
    }
  ]
}
```

The `limit` parameter caps the number of versions returned (most recent). Set to `0` or omit for unlimited.

## Point-in-Time Queries

Reconstruct a record's state at any specific timestamp:

```bash
curl http://localhost:3000/api/records/{entity_id}/at?timestamp=2026-04-03T00:00:00Z \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "user/name": "Alice",
  "user/email": "alice@example.com"
}
```

Only triples with `created_at <= timestamp` are included in the reconstruction. If the entity did not exist at the given time, a 404 error is returned.

## Get a Specific Version

```bash
curl http://localhost:3000/api/records/{entity_id}/versions/{version_number} \
  -H "Authorization: Bearer <token>"
```

Returns the complete attribute map as it existed after that version's transaction.

## Restoring to a Previous Version

Restore a record to any earlier version. This creates a **new transaction** that writes the necessary triples to bring the record back to the target state. The version history is preserved -- restoring is additive, not destructive.

```bash
curl -X POST http://localhost:3000/api/records/{entity_id}/restore \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{ "version": 1 }'
```

Response:

```json
{
  "ok": true,
  "tx_id": 63,
  "message": "Record restored to version 1"
}
```

The restore operation computes the diff between the current state and the target version, then retracts changed/removed attributes and asserts the target values. If the record is already at the requested version, a 400 error is returned.

## Undo Last Change

Shortcut to restore to version N-1:

```bash
curl -X POST http://localhost:3000/api/records/{entity_id}/undo \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "ok": true,
  "tx_id": 64
}
```

Returns a 400 error if the record has only one version (nothing to undo).

## Restoring Deleted Records

If a record was deleted (all triples retracted), it can be restored to its last known state:

```bash
curl -X POST http://localhost:3000/api/records/{entity_id}/restore-deleted \
  -H "Authorization: Bearer <token>"
```

The operation replays the full triple history to find the last version where the record had data, then re-asserts those values in a new transaction. Returns a 400 error if the record still has active triples (not actually deleted) or a 404 if no triples exist at all.

## Table-Level Snapshots

Snapshots record a checkpoint for an entire entity type at the current `tx_id`. You can later diff against or restore to a snapshot.

### Create a Snapshot

```bash
curl -X POST http://localhost:3000/api/snapshots \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "entity_type": "user",
    "name": "pre-migration",
    "description": "Snapshot before schema migration"
  }'
```

Response:

```json
{
  "id": "snapshot-uuid",
  "entity_type": "user",
  "name": "pre-migration",
  "description": "Snapshot before schema migration",
  "created_by": "user-uuid",
  "created_at": "2026-04-07T10:00:00Z",
  "record_count": 42,
  "tx_id_at_snapshot": 100
}
```

### List Snapshots

```bash
curl "http://localhost:3000/api/snapshots?entity_type=user" \
  -H "Authorization: Bearer <token>"
```

Returns snapshots in reverse chronological order.

### Diff Against a Snapshot

See what changed since a snapshot was taken:

```bash
curl http://localhost:3000/api/snapshots/{snapshot_id}/diff \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "snapshot_id": "uuid",
  "entity_type": "user",
  "snapshot_tx_id": 100,
  "current_tx_id": 150,
  "entities_created": 3,
  "entities_modified": 5,
  "entities_deleted": 1,
  "triples_added": 15,
  "triples_retracted": 4
}
```

### Restore a Snapshot

Restore all records of an entity type to their state at the snapshot's `tx_id`:

```bash
curl -X POST http://localhost:3000/api/snapshots/{snapshot_id}/restore \
  -H "Authorization: Bearer <token>"
```

For each entity:
1. Reconstruct its state at the snapshot's `tx_id` by replaying triples up to that point.
2. Compare against the current state.
3. Write new triples to bring it back to the snapshot state.

Entities created after the snapshot are fully retracted. Entities deleted after the snapshot are re-asserted. Entities already matching the snapshot state are skipped.

This is a potentially expensive operation on large datasets -- it runs within a single database transaction.

Response:

```json
{
  "ok": true,
  "tx_id": 151
}
```
