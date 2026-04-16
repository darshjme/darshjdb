# Webhooks

DarshJDB's outbound webhook system delivers signed JSON payloads to your endpoints when events occur. Webhooks support event filtering, entity-type scoping, HMAC-SHA256 signature verification, exponential-backoff retries, and a circuit breaker that auto-disables persistently failing endpoints.

## Creating a Webhook

```bash
curl -X POST http://localhost:3000/api/webhooks \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com/hook",
    "events": ["RecordCreated", "RecordUpdated"],
    "entity_types": ["User", "Order"],
    "headers": { "X-Custom": "value" },
    "retry_policy": {
      "max_retries": 5,
      "backoff_base_ms": 2000,
      "max_delay_ms": 60000
    }
  }'
```

Response (201 Created):

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "secret": "a1b2c3d4e5f6...64-hex-chars",
  "url": "https://example.com/hook",
  "active": true,
  "created_at": "2026-04-07T10:00:00Z"
}
```

The `secret` is returned **exactly once** at creation time. Store it securely -- it is needed to verify delivery signatures.

### Configuration Fields

| Field | Type | Description |
|-------|------|-------------|
| `url` | string (required) | The HTTPS endpoint to receive payloads |
| `events` | string[] | Event kinds to subscribe to. Empty = all events. |
| `entity_types` | string[] | Only deliver events for these entity types. Omit for all. |
| `headers` | object | Extra HTTP headers sent with every delivery |
| `retry_policy` | object | Override default retry behavior |

## HMAC-SHA256 Signature Verification

Every delivery includes the `X-DDB-Signature` header containing the hex-encoded HMAC-SHA256 of the raw request body, keyed with the webhook's shared secret. A unique `X-DDB-Delivery-Id` header is also sent for idempotency tracking.

### Verifying on the Receiving Side

```python
import hmac
import hashlib

def verify_signature(secret: str, body: bytes, signature: str) -> bool:
    expected = hmac.new(
        secret.encode(),
        body,
        hashlib.sha256
    ).hexdigest()
    return hmac.compare_digest(expected, signature)

# In your webhook handler:
raw_body = request.body  # raw bytes, not parsed JSON
signature = request.headers["X-DDB-Signature"]
if not verify_signature(WEBHOOK_SECRET, raw_body, signature):
    return Response(status=401)
```

```javascript
const crypto = require("crypto");

function verifySignature(secret, body, signature) {
  const expected = crypto
    .createHmac("sha256", secret)
    .update(body)
    .digest("hex");
  return crypto.timingSafeEqual(
    Buffer.from(expected),
    Buffer.from(signature)
  );
}
```

The signature is computed using constant-time comparison internally. Always use constant-time comparison on the receiving side to prevent timing attacks.

## Retry Policy

Failed deliveries are retried with exponential backoff:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_retries` | 3 | Maximum retry attempts (excludes the initial attempt) |
| `backoff_base_ms` | 1000 | Base delay in milliseconds |
| `max_delay_ms` | 30000 | Maximum delay cap |

Retry delay formula: `min(backoff_base_ms * 2^(attempt-1), max_delay_ms)`

Only 5xx responses and network errors are retried. 4xx responses are treated as permanent failures and are not retried.

## Circuit Breaker

After **10 consecutive delivery failures** across all events for a given webhook, DarshJDB automatically sets the webhook's `active` flag to `false`. The webhook stops receiving deliveries until you manually re-enable it.

To re-enable a tripped webhook:

```bash
curl -X PATCH http://localhost:3000/api/webhooks/{id} \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{ "active": true }'
```

On any successful delivery, the consecutive failure counter resets to zero.

## Delivery Tracking

Every delivery attempt is recorded in the `webhook_deliveries` table. Each record includes:

| Field | Description |
|-------|-------------|
| `id` | Unique delivery ID (also sent as `X-DDB-Delivery-Id`) |
| `webhook_id` | The webhook this delivery belongs to |
| `event` | The event kind that triggered the delivery |
| `payload` | The JSON payload that was sent |
| `status` | `pending`, `delivered`, or `failed` |
| `attempts` | Total number of attempts made |
| `last_attempt_at` | Timestamp of the most recent attempt |
| `response_status` | HTTP status code from the last response |
| `response_body` | Truncated response body (max 2048 bytes) |

## Webhook Payload Format

```json
{
  "event": "RecordCreated",
  "event_id": "uuid",
  "entity_type": "User",
  "entity_id": "uuid",
  "attribute": null,
  "old_value": null,
  "new_value": null,
  "user_id": "uuid",
  "timestamp": "2026-04-07T10:00:00Z",
  "tx_id": 42,
  "metadata": {}
}
```

## Testing Webhooks

Send a test payload to verify your endpoint is correctly receiving and verifying signatures:

```bash
curl -X POST http://localhost:3000/api/webhooks/{id}/test \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "id": "delivery-uuid",
  "webhook_id": "webhook-uuid",
  "event": "test",
  "status": "delivered",
  "attempts": 1,
  "response_status": 200,
  "response_body": "OK"
}
```

## API Reference

### List webhooks

```bash
curl http://localhost:3000/api/webhooks \
  -H "Authorization: Bearer <token>"
```

Admins see all webhooks. Non-admin users see only their own.

### Get a webhook with recent deliveries

```bash
curl http://localhost:3000/api/webhooks/{id} \
  -H "Authorization: Bearer <token>"
```

Response includes the webhook configuration and the 10 most recent delivery records.

### Update a webhook

```bash
curl -X PATCH http://localhost:3000/api/webhooks/{id} \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://new-endpoint.example.com/hook",
    "events": ["RecordDeleted"],
    "active": true,
    "entity_types": ["Order"]
  }'
```

### Delete a webhook

```bash
curl -X DELETE http://localhost:3000/api/webhooks/{id} \
  -H "Authorization: Bearer <token>"
```

Deleting a webhook cascades to remove all associated delivery records.

### List delivery attempts

```bash
curl "http://localhost:3000/api/webhooks/{id}/deliveries?limit=50" \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "deliveries": [
    {
      "id": "uuid",
      "event": "RecordCreated",
      "status": "delivered",
      "attempts": 1,
      "response_status": 200,
      "created_at": "2026-04-07T10:00:00Z"
    }
  ]
}
```
