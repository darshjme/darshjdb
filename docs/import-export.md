# Import & Export

DarshJDB supports streaming CSV and JSON import with automatic type inference, and streaming CSV/JSON export as file downloads. Large imports (>1MB) run asynchronously with job tracking.

## CSV Import

Upload a CSV file via multipart form data. Each row becomes an entity in the EAV triple store with a generated UUID.

```bash
curl -X POST http://localhost:3000/api/import/csv \
  -H "Authorization: Bearer <token>" \
  -F "file=@users.csv" \
  -F 'config={"entity_type":"user","delimiter":44,"has_header":true,"skip_errors":false,"batch_size":1000}'
```

### CSV Import Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `entity_type` | string (required) | -- | Entity type name for all imported records |
| `field_mapping` | object | `{}` | Map from CSV column name to EAV attribute name |
| `delimiter` | u8 | `44` (`,`) | CSV field delimiter as ASCII byte value |
| `has_header` | bool | `true` | Whether the first row contains column headers |
| `skip_errors` | bool | `false` | Skip rows that fail parsing instead of aborting |
| `batch_size` | usize | `1000` | Number of rows per transaction batch |

### Column Mapping

When `field_mapping` is empty, columns are mapped as `<entity_type>/<column_name>`. For example, a CSV with headers `name,age` and `entity_type: "user"` produces attributes `user/name` and `user/age`.

Explicit mapping overrides specific columns:

```json
{
  "field_mapping": {
    "name": "person/full_name",
    "email": "contact/email"
  }
}
```

Unmapped columns fall back to the default `<entity_type>/<header>` pattern.

For headerless CSVs, use numeric string keys in `field_mapping`:

```json
{
  "has_header": false,
  "field_mapping": {
    "0": "user/name",
    "1": "user/age"
  }
}
```

### Auto-Mapping

DarshJDB can automatically match CSV headers to existing EAV attributes using a three-tier fuzzy matching strategy:

1. **Exact match** (case-insensitive): header `Email` matches attribute `email`
2. **Suffix match**: header `email` matches attribute `user/email`
3. **Normalized match**: header `first_name` matches attribute `first-name` (underscores, hyphens, and spaces are collapsed)

### Type Inference

Values are automatically typed during import. The inference order (most specific first):

| Type | Detection Rule | EAV Type Tag |
|------|---------------|--------------|
| Boolean | `true`/`false`/`yes`/`no`/`1`/`0` (case-insensitive) | 3 |
| Integer | Parses as `i64` | 1 |
| Float | Parses as `f64` | 2 |
| Timestamp | RFC 3339 or `YYYY-MM-DD` format | 4 |
| Reference | 36-character valid UUID | 5 |
| JSON | Starts with `{`/`[` and parses as valid JSON | 6 |
| String | Everything else | 0 |

## JSON Import

Upload a JSON file (array or NDJSON format):

```bash
curl -X POST http://localhost:3000/api/import/json \
  -H "Authorization: Bearer <token>" \
  -F "file=@users.json" \
  -F 'config={"entity_type":"user","format":"auto","skip_errors":false,"batch_size":1000}'
```

### JSON Import Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `entity_type` | string (required) | -- | Entity type name for all imported records |
| `field_mapping` | object | `{}` | Map from JSON key to EAV attribute name |
| `format` | string | `"auto"` | Input format: `"array"`, `"ndjson"`, or `"auto"` |
| `skip_errors` | bool | `false` | Skip objects that fail parsing |
| `batch_size` | usize | `1000` | Objects per transaction batch |

### Supported Formats

**JSON Array**: Standard `[{...}, {...}]` format. Parsed all at once.

```json
[
  {"name": "Alice", "age": 30},
  {"name": "Bob", "age": 25}
]
```

**NDJSON**: One JSON object per line. Parsed line-by-line for constant memory usage on large files.

```
{"name": "Alice", "age": 30}
{"name": "Bob", "age": 25}
```

**Auto-detect**: If the first non-whitespace character is `[`, the file is treated as a JSON array. Otherwise, it is parsed as NDJSON.

### ID Handling

If an object contains an `id` field with a valid UUID string, that UUID is used as the entity ID. The `id` field itself is not stored as a separate attribute. Objects without an `id` field receive a generated UUID.

## CSV Export

Stream all records of an entity type as a CSV download:

```bash
curl -o users.csv \
  "http://localhost:3000/api/export/csv?type=user&fields=user/name,user/age&delimiter=," \
  -H "Authorization: Bearer <token>"
```

### Query Parameters

| Parameter | Description |
|-----------|-------------|
| `type` (required) | Entity type to export |
| `fields` | Comma-separated list of attributes to include. Omit for all. |
| `delimiter` | CSV delimiter character (default: `,`) |

Response headers include `X-Export-Count` (number of entities) and `X-Export-Duration-Ms`. The `Content-Disposition` header triggers a file download named `{entity_type}.csv`.

## JSON Export

Stream all records as a JSON download:

```bash
curl -o users.json \
  "http://localhost:3000/api/export/json?type=user&pretty=true&format=array" \
  -H "Authorization: Bearer <token>"
```

### Query Parameters

| Parameter | Description |
|-----------|-------------|
| `type` (required) | Entity type to export |
| `pretty` | Pretty-print JSON (default: `false`) |
| `format` | Output format: `array` (default) or `ndjson` |

For NDJSON format, the content type is `application/x-ndjson` and the file extension is `.ndjson`.

## Import Progress Tracking

Files larger than 1MB are imported asynchronously. The initial response returns a job ID:

```json
{
  "ok": true,
  "async": true,
  "job_id": "550e8400-e29b-41d4-a716-446655440000",
  "message": "Large file import started. Poll /api/import/status/{job_id} for progress."
}
```

Poll the job status:

```bash
curl http://localhost:3000/api/import/status/{job_id} \
  -H "Authorization: Bearer <token>"
```

Response while running:

```json
{
  "ok": true,
  "job_id": "550e8400-...",
  "state": "running",
  "result": null,
  "error": null
}
```

Response on completion:

```json
{
  "ok": true,
  "job_id": "550e8400-...",
  "state": "completed",
  "result": {
    "rows_processed": 50000,
    "rows_imported": 49998,
    "rows_skipped": 2,
    "triples_written": 249990,
    "duration_ms": 12340,
    "errors": [
      { "row": 1024, "message": "CSV parse error: ..." },
      { "row": 30001, "message": "CSV parse error: ..." }
    ]
  },
  "error": null
}
```

Job states: `running`, `completed`, `failed`.

## Synchronous Import Response

Files under 1MB are imported synchronously. The response includes the full result directly:

```json
{
  "ok": true,
  "async": false,
  "rows_processed": 100,
  "rows_imported": 100,
  "rows_skipped": 0,
  "triples_written": 500,
  "duration_ms": 42,
  "errors": []
}
```
