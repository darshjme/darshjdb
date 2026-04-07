# Typed Fields

Fields define the schema for entity types in DarshJDB, providing validation, type conversion, and smart casting. Each field is persisted as an entity in the triple store with type `field:{uuid}` and attributes for name, type, table binding, config, and display order.

## Field Configuration

Every field has the following properties:

| Property | Type | Description |
|---|---|---|
| `id` | UUID | Unique field identifier (auto-generated) |
| `name` | string | Human-readable field name |
| `field_type` | enum | One of the 25 supported field types |
| `table_entity_type` | string | The entity type (table) this field belongs to |
| `description` | string? | Optional description |
| `required` | bool | Whether a value is required (rejects null) |
| `unique` | bool | Whether values must be unique across entities |
| `default_value` | any? | Default value when none is provided |
| `options` | object? | Type-specific configuration (see below) |
| `order` | i32 | Display order within the table |

## All 25 Field Types

### Data Entry Fields

| Type | Key | Description | Triple Value Type |
|---|---|---|---|
| Single Line Text | `single_line_text` | Short text without newlines | String (0) |
| Long Text | `long_text` | Multi-line text with newlines | String (0) |
| Number | `number` | Numeric value with configurable precision | Float (2) |
| Checkbox | `checkbox` | Boolean true/false | Boolean (3) |
| Date | `date` | Calendar date (YYYY-MM-DD) | Timestamp (4) |
| DateTime | `date_time` | Date with time (RFC 3339) | Timestamp (4) |
| Email | `email` | Validated email address | String (0) |
| URL | `url` | Validated URL (http/https/ftp) | String (0) |
| Phone | `phone` | Phone number (digits + optional leading +) | String (0) |
| Currency | `currency` | Monetary value with symbol and precision | Float (2) |
| Percent | `percent` | Percentage value (0.0-1.0 or 0-100) | Float (2) |
| Duration | `duration` | Time duration in seconds or string | Float (2) |
| Rating | `rating` | Integer rating with configurable max | Integer (1) |
| Single Select | `single_select` | One choice from a predefined list | String (0) |
| Multi Select | `multi_select` | Multiple choices from a predefined list | Json (6) |
| Attachment | `attachment` | File attachment metadata (JSON object/array) | Json (6) |

### Relational Fields

| Type | Key | Description | Triple Value Type |
|---|---|---|---|
| Link | `link` | Reference to entities in another table | Reference (5) |
| Lookup | `lookup` | Value pulled from a linked record (computed) | Varies |
| Rollup | `rollup` | Aggregation over linked record values (computed) | Varies |

### Computed Fields

| Type | Key | Description | Triple Value Type |
|---|---|---|---|
| Formula | `formula` | Calculated from an expression | Varies |
| Auto Number | `auto_number` | Auto-incrementing integer | Integer (1) |
| Created Time | `created_time` | When the record was created | Timestamp (4) |
| Last Modified Time | `last_modified_time` | When the record was last changed | Timestamp (4) |
| Created By | `created_by` | Who created the record | String (0) |
| Last Modified By | `last_modified_by` | Who last changed the record | String (0) |

Computed fields (`auto_number`, `created_time`, `last_modified_time`, `created_by`, `last_modified_by`, `lookup`, `rollup`, `formula`) are read-only and reject user-supplied values.

## Field Options

Type-specific configuration is stored in the `options` field. The `kind` discriminator must match the field type.

### NumberOptions

```json
{
  "kind": "number",
  "precision": 2,
  "format": "decimal"
}
```

- `precision` (u8): Decimal places. 0 = integer display.
- `format` (string): Display format, e.g. `"decimal"` or `"integer"`.

### SelectOptions

Used by both `single_select` and `multi_select`:

```json
{
  "kind": "select",
  "choices": [
    { "id": "opt1", "name": "Active", "color": "#22c55e" },
    { "id": "opt2", "name": "Inactive", "color": "#ef4444" }
  ]
}
```

Each choice has an `id`, `name`, and CSS-compatible `color`.

### LinkOptions

```json
{
  "kind": "link",
  "linked_table": "task",
  "symmetric": true
}
```

- `linked_table` (string): Entity type of the linked table.
- `symmetric` (bool): Whether to create a bidirectional backlink.

### LookupOptions

```json
{
  "kind": "lookup",
  "link_field": "f1e2d3c4-...",
  "lookup_field": "f5e6d7c8-..."
}
```

- `link_field` (UUID): The link field to traverse.
- `lookup_field` (UUID): The field to read from the linked entity.

### RollupOptions

```json
{
  "kind": "rollup",
  "link_field": "f1e2d3c4-...",
  "rollup_field": "f5e6d7c8-...",
  "function": "sum"
}
```

Available functions: `count`, `sum`, `average`, `min`, `max`, `count_all`, `count_values`, `count_empty`, `array_join`.

### FormulaOptions

```json
{
  "kind": "formula",
  "expression": "IF({Status} = \"Done\", 1, 0)"
}
```

### CurrencyOptions

```json
{
  "kind": "currency",
  "symbol": "$",
  "precision": 2
}
```

### RatingOptions

```json
{
  "kind": "rating",
  "max": 5,
  "icon": "star"
}
```

## Validation Rules

The validation engine applies type-specific checks and lightweight coercion. If a field is `required`, null values are rejected. Otherwise null passes through.

### Coercion Behavior

| Field Type | Input | Coerced Output |
|---|---|---|
| SingleLineText | `42` (number) | `"42"` (string) |
| SingleLineText | `true` (bool) | `"true"` (string) |
| Number | `"123.45"` (string) | `123.45` (number) |
| Checkbox | `"yes"`, `"1"`, `"true"` | `true` |
| Checkbox | `0` (number) | `false` |
| Checkbox | `1` (number) | `true` |
| Email | `"User@Example.COM"` | `"user@example.com"` |
| Phone | `"+1 (555) 123-4567"` | `"+15551234567"` |

### Type-Specific Validation

- **SingleLineText**: Rejects strings containing newlines.
- **Email**: Must have exactly one `@` with non-empty local and domain parts; domain must contain a dot.
- **URL**: Must start with `http://`, `https://`, or `ftp://`; must have a non-empty host.
- **Phone**: Must have at least 7 digits after stripping non-digit characters.
- **Number**: Rounded to configured `precision`. String inputs are parsed.
- **Currency**: Same rounding behavior as Number, using currency-specific precision (default: 2).
- **Rating**: Must be between 0 and the configured `max` (default: 5). Rounded to integer.
- **Duration**: Must be non-negative. Accepts number (seconds) or string.
- **Date**: Accepts `YYYY-MM-DD`, `MM/DD/YYYY`, `DD.MM.YYYY`, and `YYYY/MM/DD`. Normalises to `YYYY-MM-DD`.
- **DateTime**: Accepts `T` or space separator between date and time. Date-only input gets midnight appended (`T00:00:00Z`).
- **SingleSelect**: Value must match a configured choice's `name` or `id`.
- **MultiSelect**: Must be an array of strings, each matching a configured choice.
- **Attachment**: Must be a JSON object or array.
- **Link**: Must be a UUID string or array of UUID strings.
- **Computed fields**: Reject any user-supplied value.

## Type Conversion

When a field's type changes, all existing values are batch-converted. Conversions are classified as lossless (value preserved) or lossy (value lost).

### Lossless Conversions

| From | To | Notes |
|---|---|---|
| Any type | SingleLineText / LongText | Numbers become strings, bools become "true"/"false", arrays become comma-separated |
| Number | Currency, Percent | Value passes through |
| Currency | Number, Percent | Value passes through |
| Percent | Number, Currency | Value passes through |
| Number | Checkbox | 0 = false, non-zero = true |
| Checkbox | Number | true = 1, false = 0 |
| Text | Number | Strips currency symbols and commas, parses |
| Text | Checkbox | "true"/"yes"/"1"/"on" = true; "false"/"no"/"0"/"off"/"" = false |
| Text | Date | Parses ISO format (YYYY-MM-DD) |
| Text | DateTime | Parses ISO format, date-only adds midnight |
| Date | DateTime | Appends `T00:00:00Z` |
| DateTime | Date | Truncates time portion |
| SingleSelect | MultiSelect | Wraps value in single-element array |
| MultiSelect | SingleSelect | Takes first element |
| Number | Rating | Rounds and clamps to 0-5 range |
| Rating | Number | Value passes through |
| Text | SingleSelect | Value passes through (validation at field level) |
| Text | MultiSelect | Wraps string in single-element array |
| Text | Email, URL, Phone | Value passes through |

### Lossy Conversions

Any conversion not listed above produces `null` for that value and a warning message. For example, Attachment to Number is lossy. The conversion summary reports total, success, and failed counts.

## API Endpoints

### Create a Field

```
POST /api/fields
```

```bash
curl -X POST http://localhost:4000/api/fields \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Status",
    "field_type": "single_select",
    "table_entity_type": "task",
    "required": true,
    "options": {
      "kind": "select",
      "choices": [
        {"id": "1", "name": "Todo", "color": "#gray"},
        {"id": "2", "name": "Done", "color": "#green"}
      ]
    },
    "order": 1
  }'
```

Response (201 Created):

```json
{
  "field": {
    "id": "f1e2d3c4-...",
    "name": "Status",
    "field_type": "single_select",
    "table_entity_type": "task",
    "required": true,
    "unique": false,
    "options": {
      "kind": "select",
      "choices": [
        {"id": "1", "name": "Todo", "color": "#gray"},
        {"id": "2", "name": "Done", "color": "#green"}
      ]
    },
    "order": 1
  }
}
```

### List Fields

```
GET /api/fields[?type={entity_type}]
```

```bash
curl "http://localhost:4000/api/fields?type=task"
```

Response:

```json
{
  "fields": [
    {"id": "...", "name": "Name", "field_type": "single_line_text", "order": 0},
    {"id": "...", "name": "Status", "field_type": "single_select", "order": 1}
  ]
}
```

Fields are sorted by `order` then by `name`.

### Get a Field

```
GET /api/fields/{id}
```

```bash
curl http://localhost:4000/api/fields/f1e2d3c4-...
```

### Update a Field

```
PATCH /api/fields/{id}
```

All properties are optional. If `field_type` is changed, a batch conversion of existing values is triggered automatically.

```bash
curl -X PATCH http://localhost:4000/api/fields/f1e2d3c4-... \
  -H "Content-Type: application/json" \
  -d '{
    "field_type": "single_line_text",
    "required": false
  }'
```

Response (with conversion info when type changed):

```json
{
  "field": {
    "id": "f1e2d3c4-...",
    "name": "Status",
    "field_type": "single_line_text"
  },
  "conversion": {
    "total": 42,
    "success": 40,
    "failed": 2,
    "warnings": [
      "cannot convert single_select -> single_line_text: value {\"complex\": true}"
    ]
  }
}
```

### Delete a Field

```
DELETE /api/fields/{id}
```

Deletes the field definition and retracts all entity values that use this field's attribute name.

```bash
curl -X DELETE http://localhost:4000/api/fields/f1e2d3c4-...
```

Returns 204 No Content on success.

## EAV Triple Mapping

Each field config is stored under entity `field:{uuid}` with these triples:

| Attribute | Value |
|---|---|
| `field/name` | Human-readable name |
| `field/type` | Field type discriminator string |
| `field/table` | Entity type (table) the field belongs to |
| `field/config` | Full JSON-encoded FieldConfig |
| `field/order` | Display ordering integer |

Record values are stored as triples where the attribute is the field name and the `value_type` tag maps to the field type (0=String, 1=Integer, 2=Float, 3=Boolean, 4=Timestamp, 5=Reference, 6=Json).
