# Table Management

Tables in DarshJDB provide explicit, named structure over the underlying EAV triple store. While the triple store can represent any entity shape implicitly, tables add metadata such as field ordering, display settings, templates, and behavioral controls. Table configurations are themselves stored as EAV triples (entity = `table:{uuid}`), making the system fully self-describing.

## Table Configuration

Every table has the following properties:

| Property | Type | Description |
|---|---|---|
| `id` | UUID | Unique table identifier (auto-generated) |
| `name` | string | Human-readable name (e.g. "Project Tracker") |
| `slug` | string | URL-safe slug derived from the name (e.g. "project-tracker") |
| `description` | string? | Optional description of what this table stores |
| `icon` | string? | Optional emoji icon for UI display |
| `color` | string? | Optional hex color for UI theming (e.g. "#4A90D9") |
| `primary_field` | UUID? | Which field serves as the record title / primary display |
| `field_ids` | UUID[] | Ordered list of field IDs defining column ordering |
| `default_view_id` | UUID? | Default view to show when opening this table |
| `settings` | object | Table-level behavioral settings (see below) |
| `created_at` | datetime | When the table config was created |
| `updated_at` | datetime | When the table config was last modified |

### Table Settings

Settings control table-level behavior and limits:

| Setting | Type | Default | Description |
|---|---|---|---|
| `allow_duplicates` | bool | `true` | Whether duplicate records (identical field values) are allowed |
| `enable_history` | bool | `true` | Whether to track full triple history for undo/audit |
| `max_records` | u32? | `null` | Hard limit on the number of records in this table |
| `enable_comments` | bool | `false` | Whether inline comments on records are enabled |

## API Endpoints

### Create a Table

```
POST /api/tables
```

Create a new table. The slug is auto-generated from the name. If `template` is provided, the table is pre-populated with fields and sample data from a built-in template.

```bash
curl -X POST http://localhost:4000/api/tables \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Tasks",
    "description": "Track work items",
    "icon": "T",
    "color": "#4A90D9",
    "settings": {
      "allow_duplicates": true,
      "enable_history": true,
      "max_records": 10000,
      "enable_comments": true
    }
  }'
```

Response (201 Created):

```json
{
  "table": {
    "id": "a1b2c3d4-...",
    "name": "Tasks",
    "slug": "tasks",
    "description": "Track work items",
    "icon": "T",
    "color": "#4A90D9",
    "primary_field": null,
    "field_ids": [],
    "default_view_id": null,
    "settings": {
      "allow_duplicates": true,
      "enable_history": true,
      "max_records": 10000,
      "enable_comments": true
    },
    "created_at": "2026-04-07T12:00:00Z",
    "updated_at": "2026-04-07T12:00:00Z"
  },
  "created": true
}
```

Minimal creation (only `name` is required):

```bash
curl -X POST http://localhost:4000/api/tables \
  -H "Content-Type: application/json" \
  -d '{"name": "Simple"}'
```

### List Tables

```
GET /api/tables[?include_counts=true]
```

List all table configurations, sorted alphabetically by name. Pass `include_counts=true` to include record counts per table (slower query).

```bash
curl http://localhost:4000/api/tables?include_counts=true
```

Response:

```json
{
  "tables": [
    {
      "id": "...",
      "name": "Contacts",
      "slug": "contacts",
      "record_count": 42
    },
    {
      "id": "...",
      "name": "Tasks",
      "slug": "tasks",
      "record_count": 156
    }
  ],
  "total": 2
}
```

### Get a Table

```
GET /api/tables/{id}
```

```bash
curl http://localhost:4000/api/tables/a1b2c3d4-e5f6-7890-abcd-ef1234567890
```

Response:

```json
{
  "table": {
    "id": "a1b2c3d4-...",
    "name": "Tasks",
    "slug": "tasks",
    "description": "Track work items",
    "icon": "T",
    "color": "#4A90D9",
    "primary_field": "f1e2d3c4-...",
    "field_ids": ["f1e2d3c4-...", "f5e6d7c8-..."],
    "settings": { "allow_duplicates": true, "enable_history": true, "max_records": null, "enable_comments": false },
    "created_at": "2026-04-07T12:00:00Z",
    "updated_at": "2026-04-07T14:30:00Z"
  }
}
```

### Update a Table

```
PATCH /api/tables/{id}
```

Partial updates. Changing the `name` also regenerates the `slug`. To clear an optional field, send `null`.

```bash
curl -X PATCH http://localhost:4000/api/tables/a1b2c3d4-... \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Active Tasks",
    "description": null,
    "settings": {
      "allow_duplicates": false,
      "enable_history": true,
      "max_records": 5000,
      "enable_comments": true
    }
  }'
```

### Delete a Table

```
DELETE /api/tables/{id}
```

Deletes the table configuration and cascades to all records belonging to the table. Records are identified by their `:db/type` triple matching the table's slug.

```bash
curl -X DELETE http://localhost:4000/api/tables/a1b2c3d4-...
```

Response:

```json
{
  "deleted": true,
  "table_id": "a1b2c3d4-..."
}
```

### Duplicate a Table

```
POST /api/tables/{id}/duplicate
```

Duplicate a table's structure (field definitions, settings, icon, color). Optionally copy all records.

```bash
curl -X POST http://localhost:4000/api/tables/a1b2c3d4-.../duplicate \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Tasks Archive",
    "include_data": true
  }'
```

Response (201 Created):

```json
{
  "table": {
    "id": "new-uuid-...",
    "name": "Tasks Archive",
    "slug": "tasks-archive"
  },
  "duplicated_from": "a1b2c3d4-...",
  "include_data": true
}
```

When `include_data` is true, all record triples from the source table are copied with new entity IDs and remapped attribute prefixes (e.g. `tasks/status` becomes `tasks-archive/status`).

### Get Table Statistics

```
GET /api/tables/{id}/stats
```

```bash
curl http://localhost:4000/api/tables/a1b2c3d4-.../stats
```

Response:

```json
{
  "table_id": "a1b2c3d4-...",
  "stats": {
    "record_count": 156,
    "field_count": 7,
    "triple_count": 1092,
    "last_modified": "2026-04-07T14:30:00Z"
  }
}
```

## Built-in Templates

Templates provide quick-start table creation with pre-configured fields and optional sample data. Pass the template name in the `template` field of the create request.

```bash
curl -X POST http://localhost:4000/api/tables \
  -H "Content-Type: application/json" \
  -d '{"name": "My Projects", "template": "project_tracker"}'
```

Template lookup is case-insensitive.

### project_tracker

Track projects with status, priority, and assignments.

| Field | Type | Options |
|---|---|---|
| Name | text | (primary field) |
| Status | select | Todo, In Progress, Done |
| Priority | select | Low, Medium, High, Urgent |
| Assignee | text | |
| Due Date | date | |
| Description | multiline | |

Includes 2 sample records.

### contacts

Manage contacts with emails, phones, and company info.

| Field | Type |
|---|---|
| Name | text |
| Email | email |
| Phone | text |
| Company | text |
| Tags | text |
| Notes | multiline |
| Last Contact | date |

Includes 1 sample record.

### inventory

Track products, SKUs, quantities, and suppliers.

| Field | Type | Options |
|---|---|---|
| Product Name | text | |
| SKU | text | |
| Category | select | Electronics, Clothing, Food, Other |
| Quantity | number | |
| Price | number | precision: 2, prefix: "$" |
| Supplier | text | |
| Reorder Level | number | |

Includes 1 sample record.

### content_calendar

Plan and schedule content publication.

| Field | Type | Options |
|---|---|---|
| Title | text | |
| Status | select | Draft, Review, Scheduled, Published |
| Author | text | |
| Publish Date | date | |
| Category | select | Blog, Social, Newsletter, Video |
| URL | url | |
| Notes | multiline | |

No sample data.

### bug_tracker

Track bugs with severity, status, and reproduction steps.

| Field | Type | Options |
|---|---|---|
| Title | text | |
| Severity | select | Critical, High, Medium, Low |
| Status | select | Open, In Progress, Fixed, Closed, Wont Fix |
| Reporter | text | |
| Assignee | text | |
| Steps to Reproduce | multiline | |
| Environment | text | |

Includes 1 sample record.

## Table Migration

Three structural migration operations are available for evolving tables beyond simple CRUD.

### Rename

Rename a table, updating its config and all record `:db/type` references atomically. The operation rewrites attribute prefixes from `old_slug/field` to `new_slug/field` across all records.

If the new name produces the same slug as the old one (e.g. capitalisation change only), only the display name is updated.

### Merge

Merge all records from a source table into a target table. Records from the source are re-typed to the target's slug and their attribute prefixes are rewritten. The source table config is deleted after migration. Fields that exist in the source but not the target are preserved as-is through the triple store.

Cannot merge a table into itself.

### Split

Split a table into multiple new tables based on distinct values of a specified field. For each unique value, a new table is created with the name format `"{original} - {value}"`. Records with no value for the split field are placed in a table suffixed with "Uncategorized". The original table is deleted after the split.

## Internal Storage

Table configs are stored as entities in the triple store with the following triples:

| Attribute | Value | Type |
|---|---|---|
| `:db/type` | `"__table"` | String |
| `table/name` | table name | String |
| `table/slug` | URL-safe slug | String |
| `table/config` | full JSON config | Json |

Records belonging to a table have `:db/type` set to the table's slug. Record field values are stored as triples with attributes prefixed by the slug (e.g. `tasks/status`, `tasks/priority`).
