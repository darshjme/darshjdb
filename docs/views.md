# Views

Views define named, reusable lenses over an entity type. Each view has its own filters, sorts, field ordering, and display configuration. Views are stored as EAV triples in the triple store, so they participate in the same transaction, audit, and reactive infrastructure as regular data.

## View Types

| Type | Key | Description |
|---|---|---|
| Grid | `grid` | Spreadsheet-style tabular layout with sortable columns |
| Form | `form` | Single-record entry form |
| Kanban | `kanban` | Card-based board grouped by a select field |
| Gallery | `gallery` | Card grid showing thumbnails and key fields |
| Calendar | `calendar` | Date-based calendar layout |

## View Configuration

Every view has the following properties:

| Property | Type | Description |
|---|---|---|
| `id` | UUID | Unique view identifier |
| `name` | string | Human-readable name |
| `kind` | enum | One of: `grid`, `form`, `kanban`, `gallery`, `calendar` |
| `table_entity_type` | string | The entity type this view queries |
| `filters` | FilterClause[] | Built-in filter predicates always applied |
| `sorts` | SortClause[] | Default sort order |
| `field_order` | string[] | Ordered list of visible field names |
| `hidden_fields` | string[] | Fields excluded from display |
| `group_by` | string? | Field for Grid view grouping |
| `kanban_field` | string? | Select field used as Kanban column key |
| `calendar_field` | string? | Date/datetime field for Calendar layout |
| `color_field` | string? | Field whose value determines row/card color |
| `row_height` | u32? | Pixel height of each row (Grid view) |
| `created_by` | UUID | User who created this view |
| `created_at` | datetime | Creation timestamp |
| `updated_at` | datetime | Last modification timestamp |

### Filter Clause

```json
{
  "field": "status",
  "op": "neq",
  "value": "archived"
}
```

Available operators:

| Operator | Key | Description |
|---|---|---|
| Equals | `eq` | Exact match |
| Not Equals | `neq` | Not equal |
| Greater Than | `gt` | Numeric/string comparison |
| Greater or Equal | `gte` | Numeric/string comparison |
| Less Than | `lt` | Numeric/string comparison |
| Less or Equal | `lte` | Numeric/string comparison |
| Contains | `contains` | Substring match |
| Is Empty | `is_empty` | Value is null (value field ignored) |
| Is Not Empty | `is_not_empty` | Value is not null (value field ignored) |

### Sort Clause

```json
{
  "field": "priority",
  "direction": "asc"
}
```

Directions: `asc`, `desc`.

## API Endpoints

All view endpoints require authentication via Bearer token.

### Create a View

```
POST /api/views
```

```bash
curl -X POST http://localhost:4000/api/views \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Active Tasks",
    "kind": "kanban",
    "table_entity_type": "Task",
    "kanban_field": "status",
    "filters": [
      {"field": "archived", "op": "eq", "value": false}
    ],
    "sorts": [
      {"field": "priority", "direction": "asc"}
    ],
    "field_order": ["title", "status", "priority", "assignee"],
    "hidden_fields": ["internal_id"]
  }'
```

Response:

```json
{
  "data": {
    "id": "v1e2f3g4-...",
    "name": "Active Tasks",
    "kind": "kanban",
    "table_entity_type": "Task",
    "kanban_field": "status",
    "filters": [{"field": "archived", "op": "eq", "value": false}],
    "sorts": [{"field": "priority", "direction": "asc"}],
    "field_order": ["title", "status", "priority", "assignee"],
    "hidden_fields": ["internal_id"],
    "created_by": "user-uuid-...",
    "created_at": "2026-04-07T12:00:00Z",
    "updated_at": "2026-04-07T12:00:00Z"
  },
  "meta": {"created": true}
}
```

### List Views

```
GET /api/views?type={entity_type}
```

The `type` query parameter is required. Views are returned sorted by `created_at` ascending.

```bash
curl "http://localhost:4000/api/views?type=Task" \
  -H "Authorization: Bearer $TOKEN"
```

Response:

```json
{
  "data": [
    {"id": "...", "name": "All Tasks", "kind": "grid"},
    {"id": "...", "name": "Active Tasks", "kind": "kanban"}
  ],
  "meta": {"count": 2}
}
```

### Get a View

```
GET /api/views/{id}
```

```bash
curl http://localhost:4000/api/views/v1e2f3g4-... \
  -H "Authorization: Bearer $TOKEN"
```

### Update a View

```
PATCH /api/views/{id}
```

All properties are optional. Only provided fields are updated.

```bash
curl -X PATCH http://localhost:4000/api/views/v1e2f3g4-... \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Renamed View",
    "filters": [
      {"field": "done", "op": "eq", "value": true}
    ],
    "row_height": 48
  }'
```

To clear an optional field (e.g. remove the kanban_field), send `null`:

```json
{"kanban_field": null}
```

### Delete a View

```
DELETE /api/views/{id}
```

```bash
curl -X DELETE http://localhost:4000/api/views/v1e2f3g4-... \
  -H "Authorization: Bearer $TOKEN"
```

Response:

```json
{
  "data": null,
  "meta": {"deleted": true}
}
```

### Query Through a View

```
POST /api/views/{id}/query
```

Execute a query through the view's lens. The view's filters and sorts are merged with any user-supplied query. Hidden fields are stripped from the result and field ordering is applied.

**With no additional query** (returns all records matching the view's filters):

```bash
curl -X POST http://localhost:4000/api/views/v1e2f3g4-.../query \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}'
```

**With additional user filters**:

```bash
curl -X POST http://localhost:4000/api/views/v1e2f3g4-.../query \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": {
      "$type": "Task",
      "$where": [
        {"title": {"$contains": "urgent"}}
      ],
      "$limit": 50
    }
  }'
```

Response:

```json
{
  "data": [
    {
      "entity_id": "record-uuid-...",
      "attributes": {
        "title": "Fix urgent bug",
        "status": "open",
        "priority": 1
      }
    }
  ],
  "meta": {
    "view_id": "v1e2f3g4-...",
    "view_name": "Active Tasks",
    "view_kind": "kanban",
    "count": 1,
    "duration_ms": 12.5
  }
}
```

## How View Filters Merge with User Query Filters

When querying through a view, filters are combined as follows:

1. **View filters are prepended** to the query's WHERE clauses. They act as mandatory base constraints that cannot be overridden.
2. **User filters are appended** after the view's filters, further narrowing the result set.
3. All filters are evaluated with AND semantics.

For example, if a view has `status != "archived"` and the user queries for `title contains "bug"`, the effective query is:

```
WHERE status != "archived" AND title CONTAINS "bug"
```

### Sort Precedence

- If the user query specifies its own sort order, **the user's sort takes priority** and the view's default sort is ignored.
- If the user query has no sort, the view's sort order is applied.

### Field Projection

After query execution, the view's display configuration is applied:

1. Fields listed in `hidden_fields` are removed from each result row.
2. If `field_order` is non-empty, fields are reordered to match (ordered fields first, then remaining non-hidden fields appended).
3. If both `hidden_fields` and `field_order` are empty, results are returned in their natural order.

## Internal Storage

Each view occupies a single entity `view:{uuid}` in the triple store with these attributes:

| Attribute | Value |
|---|---|
| `view/id` | View UUID as string |
| `view/name` | Human-readable name |
| `view/kind` | Variant tag (grid/form/kanban/gallery/calendar) |
| `view/table` | Entity type this view queries |
| `view/config` | JSON blob with filters, sorts, field_order, hidden_fields, and display settings |
| `view/created_by` | UUID of the creating user |
| `view/created_at` | ISO-8601 timestamp |
| `view/updated_at` | ISO-8601 timestamp |

The entity ID is derived deterministically from the view UUID using a SHA-256 hash with the prefix `darshjdb:view:{id}`, truncated to 16 bytes with UUID v4 variant bits set.
