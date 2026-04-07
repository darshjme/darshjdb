# Automation Engine

DarshJDB's automation engine lets you define event-driven workflows that react to data changes, scheduled times, or external signals. Each automation is a **workflow** consisting of a **trigger** and an ordered sequence of **action steps** executed as a DAG.

## Trigger Types

Every workflow starts with exactly one trigger. The `TriggerKind` enum defines what fires it:

| Trigger | Description | Requires `table_entity_type` |
|---------|-------------|------------------------------|
| `on_record_create` | Fires when a new record is created | Yes |
| `on_record_update` | Fires when any record is updated | Yes |
| `on_record_delete` | Fires when a record is deleted | Yes |
| `on_field_change` | Fires when a specific field changes on any record. Requires `field` parameter. | Yes |
| `on_schedule` | Fires on a cron schedule (7-field format: sec min hour day month weekday year) | Optional |
| `on_webhook` | Fires when an external webhook hits the automation endpoint | No |
| `on_form_submit` | Fires when a form submission creates a record | Yes |
| `manual` | Only fires via the API -- never matches events automatically | No |

### Trigger Conditions

Every trigger supports an optional `condition` array of `WhereClause` objects. When present, the trigger only fires if **all** clauses match (AND semantics). Supported operators: `eq`, `neq`, `gt`, `gte`, `lt`, `lte`, `contains`, `like`.

```json
{
  "kind": { "type": "on_record_create" },
  "table_entity_type": "orders",
  "condition": [
    { "attribute": "amount", "op": "gt", "value": 100 }
  ],
  "enabled": true
}
```

## Action Types

Each workflow step executes one action. The `ActionKind` enum defines the built-in types:

| Action | Config Keys | Description |
|--------|-------------|-------------|
| `create_record` | `entity_type`, `data` | Create a new record in the triple store |
| `update_record` | `entity_id` (or from context), `data` | Update an existing record |
| `delete_record` | `entity_id` (or from context) | Delete a record |
| `send_webhook` | `url`, `method`, `headers`, `payload` | Send an HTTP request to an external URL |
| `send_email` | `to`, `subject`, `body` | Queue an email notification |
| `run_function` | `function_name`, `args` | Execute a registered server-side function |
| `set_field_value` | `field`, `value` | Set a specific field on the triggering record |
| `add_to_view` | `view_id` | Add the triggering record to a view/collection |
| `notify` | `channel`, `message`, `recipients` | Send an in-app notification |
| `custom` | `name` + arbitrary config | Extensibility point for plugin-provided actions |

Every action has a configurable `timeout_ms` (default: 30,000ms).

## Workflow DAG

A `Workflow` ties together a trigger and an ordered list of `WorkflowStep` objects:

```json
{
  "name": "Notify on large order",
  "description": "Send Slack webhook when an order exceeds $100",
  "trigger": {
    "kind": { "type": "on_record_create" },
    "table_entity_type": "orders",
    "condition": [{ "attribute": "amount", "op": "gt", "value": 100 }],
    "enabled": true
  },
  "steps": [
    {
      "id": "notify_slack",
      "action": {
        "kind": { "type": "send_webhook" },
        "config": {
          "url": "https://hooks.slack.com/services/T.../B.../...",
          "method": "POST",
          "payload": { "text": "Large order received" }
        },
        "timeout_ms": 10000
      },
      "on_error": "continue"
    },
    {
      "id": "set_flagged",
      "action": {
        "kind": { "type": "set_field_value" },
        "config": { "field": "flagged", "value": true }
      },
      "on_error": "stop"
    }
  ],
  "enabled": true
}
```

### Step Dependencies

Steps execute sequentially by default. Each step can specify `depends_on` (a list of step IDs) for explicit dependency ordering. Independent steps with satisfied dependencies can be parallelized by the engine.

### Conditional Execution

Each step supports an optional `condition` array of `WhereClause` objects. If the condition evaluates to `false`, the step is **skipped** (not failed), and execution continues to the next step. Conditions are evaluated against `record_data` and `previous_outputs` from earlier steps.

### Step Output Chaining

Each step's output is stored in the `ActionContext.previous_outputs` map, keyed by both the step's `id` and `step_{index}`. Subsequent steps can reference earlier outputs through the context.

## Error Strategies

Each step defines an `on_error` strategy:

| Strategy | Behavior |
|----------|----------|
| `stop` (default) | Halt the entire workflow immediately |
| `continue` | Log the failure and proceed to the next step |
| `retry` | Retry the step with exponential backoff (500ms base, doubling). Specify `max_retries`. Falls through to `stop` if all retries fail. |

## Execution History and Logging

Every workflow execution produces a `WorkflowRun` record:

```json
{
  "id": "run-uuid",
  "workflow_id": "workflow-uuid",
  "trigger_event": { ... },
  "started_at": "2026-04-07T10:00:00Z",
  "completed_at": "2026-04-07T10:00:01Z",
  "status": "completed",
  "step_results": [
    {
      "step_id": "notify_slack",
      "skipped": false,
      "result": {
        "success": true,
        "output": { "action": "send_webhook", "status": 200 },
        "duration_ms": 342
      },
      "started_at": "2026-04-07T10:00:00Z",
      "completed_at": "2026-04-07T10:00:00.342Z"
    }
  ],
  "duration_ms": 450
}
```

Run statuses: `running`, `completed`, `failed`, `cancelled`.

## API Endpoints

### Create a workflow

```bash
curl -X POST http://localhost:3000/api/automations \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Welcome email",
    "trigger": {
      "kind": { "type": "on_record_create" },
      "table_entity_type": "users"
    },
    "steps": [
      {
        "id": "send_welcome",
        "action": {
          "kind": { "type": "send_email" },
          "config": {
            "to": "{{record.email}}",
            "subject": "Welcome!",
            "body": "Thanks for signing up."
          }
        }
      }
    ]
  }'
```

### List workflows

```bash
curl http://localhost:3000/api/automations \
  -H "Authorization: Bearer <token>"
```

### Get a workflow

```bash
curl http://localhost:3000/api/automations/{workflow_id} \
  -H "Authorization: Bearer <token>"
```

### Update a workflow

```bash
curl -X PATCH http://localhost:3000/api/automations/{workflow_id} \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{ "enabled": false }'
```

### Delete a workflow

```bash
curl -X DELETE http://localhost:3000/api/automations/{workflow_id} \
  -H "Authorization: Bearer <token>"
```

### Manually trigger a workflow

```bash
curl -X POST http://localhost:3000/api/automations/{workflow_id}/trigger \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{ "entity_type": "orders", "record_data": { "amount": 500 } }'
```

### Get run history

```bash
curl http://localhost:3000/api/automations/{workflow_id}/runs?limit=20 \
  -H "Authorization: Bearer <token>"
```
