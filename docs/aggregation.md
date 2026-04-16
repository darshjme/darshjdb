# Aggregation Engine

DarshJDB's aggregation engine provides GROUP BY, pivot, statistical aggregation, and time-series bucketing over the EAV triple store. It powers view-level summaries, rollup computations, and dashboard widgets.

The engine translates high-level aggregation queries into CTE-based SQL that first materializes EAV triples into a columnar form (pivot), then applies PostgreSQL's native aggregate functions. This pushes all heavy computation into the database, avoiding large client-side data transfers.

## AggregateQuery Structure

An aggregation query has five components:

```json
{
  "entity_type": "Order",
  "group_by": ["status"],
  "aggregations": [
    {"field": "amount", "function": {"fn": "Sum"}, "alias": "total_amount"},
    {"field": "id", "function": {"fn": "Count"}, "alias": "order_count"}
  ],
  "filters": [
    {"attribute": "region", "op": "Eq", "value": "US"}
  ],
  "having": {
    "alias": "total_amount",
    "op": "Gt",
    "value": 1000
  }
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `entity_type` | string | Yes | Entity type to aggregate (e.g. "Order") |
| `group_by` | string[] | No | Attributes to group by (empty = grand total) |
| `aggregations` | Aggregation[] | Yes | At least one aggregation is required |
| `filters` | WhereClause[] | No | Pre-aggregation filters (WHERE) |
| `having` | HavingClause? | No | Post-aggregation filter (HAVING) |

Each `Aggregation` has:

| Field | Type | Description |
|---|---|---|
| `field` | string | The attribute to aggregate over (must not be empty) |
| `function` | AggFn | The aggregate function to apply |
| `alias` | string | Output name for the result column (must not be empty) |

## All 18 Aggregate Functions

### Counting Functions

| Function | JSON Key | Description |
|---|---|---|
| Count | `{"fn": "Count"}` | Count of non-null values |
| Count Distinct | `{"fn": "CountDistinct"}` | Count of distinct non-null values |
| Count Empty | `{"fn": "CountEmpty"}` | Count of null or empty string values |
| Count Filled | `{"fn": "CountFilled"}` | Count of non-null, non-empty values |
| Percent Empty | `{"fn": "PercentEmpty"}` | Percentage of empty values (0-100, 2 decimal places) |
| Percent Filled | `{"fn": "PercentFilled"}` | Percentage of filled values (0-100, 2 decimal places) |

### Numeric Aggregation

| Function | JSON Key | Description |
|---|---|---|
| Sum | `{"fn": "Sum"}` | Numeric sum (casts values to numeric) |
| Avg | `{"fn": "Avg"}` | Arithmetic mean |
| Min | `{"fn": "Min"}` | Minimum value |
| Max | `{"fn": "Max"}` | Maximum value |
| StdDev | `{"fn": "StdDev"}` | Population standard deviation |
| Variance | `{"fn": "Variance"}` | Population variance |
| Median | `{"fn": "Median"}` | 50th percentile |
| Percentile | `{"fn": "Percentile", "arg": 0.95}` | Arbitrary percentile (0.0 to 1.0) |

### Positional and Collection

| Function | JSON Key | Description |
|---|---|---|
| First | `{"fn": "First"}` | First value by entity_id ordering |
| Last | `{"fn": "Last"}` | Last value by entity_id ordering (descending) |
| Array Agg | `{"fn": "ArrayAgg"}` | Collect all values into a JSON array |
| String Agg | `{"fn": "StringAgg", "arg": ", "}` | Concatenate string values with a separator |

### Validation

- `Percentile` validates that the argument is between 0.0 and 1.0.
- Every aggregation must have a non-empty `field` and `alias`.
- `having.alias` must reference an existing aggregation alias.

## GROUP BY Queries

### No Group By (Grand Total)

When `group_by` is empty, the entire result set is treated as a single group:

```bash
curl -X POST http://localhost:4000/api/aggregate \
  -H "Content-Type: application/json" \
  -d '{
    "entity_type": "Order",
    "aggregations": [
      {"field": "amount", "function": {"fn": "Sum"}, "alias": "total"},
      {"field": "id", "function": {"fn": "Count"}, "alias": "count"}
    ]
  }'
```

Response:

```json
{
  "groups": [
    {
      "key": {"_all": true},
      "values": {"total": 90000, "count": 500},
      "count": 500
    }
  ],
  "totals": {"total": 90000, "count": 500}
}
```

### Single Group By

```bash
curl -X POST http://localhost:4000/api/aggregate \
  -H "Content-Type: application/json" \
  -d '{
    "entity_type": "Order",
    "group_by": ["status"],
    "aggregations": [
      {"field": "amount", "function": {"fn": "Sum"}, "alias": "total_amount"},
      {"field": "amount", "function": {"fn": "Avg"}, "alias": "avg_amount"},
      {"field": "id", "function": {"fn": "Count"}, "alias": "order_count"}
    ]
  }'
```

Response:

```json
{
  "groups": [
    {
      "key": {"status": "completed"},
      "values": {"total_amount": 45000, "avg_amount": 375, "order_count": 120},
      "count": 120
    },
    {
      "key": {"status": "pending"},
      "values": {"total_amount": 25000, "avg_amount": 250, "order_count": 100},
      "count": 100
    }
  ],
  "totals": {"total_amount": 90000, "avg_amount": 300, "order_count": 500}
}
```

Groups are ordered by `group_count DESC` by default.

### Multi-Column Group By

```bash
curl -X POST http://localhost:4000/api/aggregate \
  -H "Content-Type: application/json" \
  -d '{
    "entity_type": "Order",
    "group_by": ["region", "status"],
    "aggregations": [
      {"field": "amount", "function": {"fn": "Count"}, "alias": "cnt"}
    ]
  }'
```

Response:

```json
{
  "groups": [
    {
      "key": {"region": "US", "status": "active"},
      "values": {"cnt": 42},
      "count": 42
    }
  ],
  "totals": {"cnt": 500}
}
```

Multi-column group keys are stored internally with `|||` separators and reassembled into structured objects in the response.

## HAVING Clause

Filter on aggregated values after grouping:

```bash
curl -X POST http://localhost:4000/api/aggregate \
  -H "Content-Type: application/json" \
  -d '{
    "entity_type": "Order",
    "group_by": ["status"],
    "aggregations": [
      {"field": "amount", "function": {"fn": "Sum"}, "alias": "total"}
    ],
    "having": {
      "alias": "total",
      "op": "Gt",
      "value": 1000
    }
  }'
```

HAVING operators: `Eq`, `Neq`, `Gt`, `Gte`, `Lt`, `Lte`.

The `alias` in the HAVING clause must match one of the aggregation aliases. This is validated before execution.

## Pre-Aggregation Filters

WHERE-style filters are applied before aggregation. Each filter produces a correlated subquery that checks the attribute value:

```bash
curl -X POST http://localhost:4000/api/aggregate \
  -H "Content-Type: application/json" \
  -d '{
    "entity_type": "Order",
    "group_by": ["status"],
    "aggregations": [
      {"field": "amount", "function": {"fn": "Sum"}, "alias": "total"}
    ],
    "filters": [
      {"attribute": "region", "op": "Eq", "value": "US"},
      {"attribute": "year", "op": "Gte", "value": "2025"}
    ]
  }'
```

Filter operators: `Eq`, `Neq`, `Gt`, `Gte`, `Lt`, `Lte`, `Contains`, `Like`.

## Time-Series Chart Queries

The chart module generates bucketed time-series data for dashboard charts using PostgreSQL's `date_trunc`.

### Chart Query Structure

```json
{
  "entity_type": "Order",
  "date_field": "created_at",
  "value_field": "amount",
  "function": "sum",
  "bucket": "month",
  "group_by": "region",
  "filters": []
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `entity_type` | string | Yes | Entity type to chart |
| `date_field` | string | Yes | Attribute holding the date/timestamp |
| `value_field` | string | Yes | Attribute whose values are aggregated |
| `function` | enum | Yes | One of: `count`, `sum`, `avg`, `min`, `max` |
| `bucket` | enum | Yes | Time interval: `day`, `week`, `month`, `quarter`, `year` |
| `group_by` | string? | No | Optional attribute for multiple series |
| `filters` | WhereClause[] | No | Pre-aggregation filters |

### Time Buckets

| Bucket | Key | PostgreSQL Interval |
|---|---|---|
| Day | `day` | `date_trunc('day', ...)` |
| Week | `week` | `date_trunc('week', ...)` |
| Month | `month` | `date_trunc('month', ...)` |
| Quarter | `quarter` | `date_trunc('quarter', ...)` |
| Year | `year` | `date_trunc('year', ...)` |

### Chart API Endpoint

```
POST /api/aggregate/chart
```

```bash
curl -X POST http://localhost:4000/api/aggregate/chart \
  -H "Content-Type: application/json" \
  -d '{
    "entity_type": "Order",
    "date_field": "created_at",
    "value_field": "amount",
    "function": "sum",
    "bucket": "month",
    "group_by": "region"
  }'
```

Response:

```json
{
  "buckets": [
    {
      "label": "2025-01-01",
      "start": "2025-01-01T00:00:00Z",
      "end": "2025-02-01T00:00:00Z",
      "value": 15000,
      "series": "US"
    },
    {
      "label": "2025-01-01",
      "start": "2025-01-01T00:00:00Z",
      "end": "2025-02-01T00:00:00Z",
      "value": 8000,
      "series": "EU"
    },
    {
      "label": "2025-02-01",
      "start": "2025-02-01T00:00:00Z",
      "end": "2025-03-01T00:00:00Z",
      "value": 18000,
      "series": "US"
    }
  ]
}
```

When `group_by` is omitted, the `series` field is absent from each bucket.

Buckets are ordered by `bucket_start ASC`, then by `series_name ASC` when grouped.

## Summary Endpoint

A convenience endpoint that computes count, sum, avg, min, max, count_empty, and count_filled for every attribute of an entity type in a single query.

```
POST /api/aggregate/summary
```

```bash
curl -X POST http://localhost:4000/api/aggregate/summary \
  -H "Content-Type: application/json" \
  -d '{"entity_type": "Invoice"}'
```

Response:

```json
{
  "groups": [
    {
      "key": {"attribute": "amount"},
      "values": {
        "count": 150,
        "count_distinct": 120,
        "count_empty": 5,
        "count_filled": 145,
        "sum": 75000,
        "avg": 500,
        "min": 10,
        "max": 5000
      },
      "count": 150
    },
    {
      "key": {"attribute": "status"},
      "values": {
        "count": 150,
        "count_distinct": 4,
        "count_empty": 0,
        "count_filled": 150,
        "sum": null,
        "avg": null,
        "min": null,
        "max": null
      },
      "count": 150
    }
  ],
  "totals": {}
}
```

Numeric aggregates (`sum`, `avg`, `min`, `max`) return `null` for non-numeric attributes.

## How CTE-Based SQL Is Generated

The SQL builder generates a three-stage CTE pipeline:

### Stage 1: Entity Discovery

```sql
WITH entity_ids AS (
  SELECT DISTINCT entity_id
  FROM triples
  WHERE attribute = ':db/type'
    AND value = to_jsonb($1::text)
    AND NOT retracted
    -- correlated subqueries for each WHERE filter
)
```

Each pre-aggregation filter adds a correlated subquery:

```sql
AND entity_id IN (
  SELECT entity_id FROM triples
  WHERE attribute = 'status'
    AND NOT retracted
    AND value = to_jsonb($2::text)
)
```

### Stage 2: EAV Pivot

```sql
pivoted AS (
  SELECT e.entity_id,
    status.value AS status_val,
    amount.value AS amount_val
  FROM entity_ids e
  LEFT JOIN LATERAL (
    SELECT value FROM triples
    WHERE entity_id = e.entity_id
      AND attribute = 'status'
      AND NOT retracted
    ORDER BY tx_id DESC LIMIT 1
  ) status ON true
  LEFT JOIN LATERAL (
    SELECT value FROM triples
    WHERE entity_id = e.entity_id
      AND attribute = 'amount'
      AND NOT retracted
    ORDER BY tx_id DESC LIMIT 1
  ) amount ON true
)
```

Each required attribute gets a `LEFT JOIN LATERAL` subquery that fetches the latest non-retracted value (ordered by `tx_id DESC LIMIT 1`). This materializes the EAV representation into a columnar form that PostgreSQL's aggregate functions can operate on.

### Stage 3: Aggregation

```sql
SELECT
  COALESCE(status_val::text, 'null') AS group_key,
  jsonb_build_object(
    'total_amount', SUM((amount_val#>>'{}')::numeric),
    'order_count', COUNT(amount_val)
  ) AS agg_values,
  COUNT(*) AS group_count
FROM pivoted
GROUP BY status_val
HAVING SUM((amount_val#>>'{}')::numeric) > 1000
ORDER BY group_count DESC
```

The aggregated values are packed into a single `jsonb_build_object` call. Group keys for multi-column groups use `|||` as a separator.

### SQL Injection Prevention

- All user-supplied values are parameterized (`$1`, `$2`, ...).
- Attribute names are sanitized to allow only alphanumeric characters, underscore, forward slash, colon, hyphen, and dot.
- Single quotes in `StringAgg` separators are escaped (`'` becomes `''`).

## API Endpoints Summary

| Method | Path | Description |
|---|---|---|
| POST | `/api/aggregate` | Execute a full aggregation query |
| POST | `/api/aggregate/summary` | Quick summary for all fields of an entity type |
| POST | `/api/aggregate/chart` | Time-series bucketed aggregation for charts |
