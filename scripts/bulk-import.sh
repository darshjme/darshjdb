#!/bin/bash
# ---------------------------------------------------------------------------
# DarshanDB Bulk Import Script
#
# Imports a JSON file of entities directly into Postgres via the DarshanDB
# REST API bulk-load endpoint, which uses UNNEST-based insertion for
# 10-50x faster throughput than batched /api/mutate.
#
# Usage:
#   ./scripts/bulk-import.sh <data.json> [SERVER_URL] [TOKEN]
#
# Arguments:
#   data.json    JSON file containing entities to import.
#                Format: { "entities": [{ "type": "...", "data": {...} }, ...] }
#                Or: array shorthand [{ "type": "...", "data": {...} }, ...]
#   SERVER_URL   DarshanDB server URL (default: http://localhost:7700)
#   TOKEN        Bearer token for authentication (default: reads DARSHAN_TOKEN env)
#
# Environment:
#   DARSHAN_TOKEN  Bearer token (used if TOKEN arg is not provided)
#   DARSHAN_URL    Server URL (used if SERVER_URL arg is not provided)
#
# Examples:
#   # Import with defaults (localhost:7700, DARSHAN_TOKEN env var)
#   ./scripts/bulk-import.sh users.json
#
#   # Import to a specific server with token
#   ./scripts/bulk-import.sh users.json http://db.example.com:7700 my-token
#
#   # Pipe from stdin
#   cat users.json | ./scripts/bulk-import.sh -
# ---------------------------------------------------------------------------

set -euo pipefail

# ── Argument parsing ──────────────────────────────────────────────────

DATA_FILE="${1:?Usage: $0 <data.json> [SERVER_URL] [TOKEN]}"
SERVER_URL="${2:-${DARSHAN_URL:-http://localhost:7700}}"
TOKEN="${3:-${DARSHAN_TOKEN:-}}"

# ── Read input ────────────────────────────────────────────────────────

if [ "$DATA_FILE" = "-" ]; then
    INPUT=$(cat)
else
    if [ ! -f "$DATA_FILE" ]; then
        echo "Error: File not found: $DATA_FILE" >&2
        exit 1
    fi
    INPUT=$(cat "$DATA_FILE")
fi

# ── Normalize input format ────────────────────────────────────────────
# Accept both { "entities": [...] } and bare array [...]

FIRST_CHAR=$(echo "$INPUT" | head -c 1 | tr -d '[:space:]')

if [ "$FIRST_CHAR" = "[" ]; then
    # Bare array — wrap in the expected envelope.
    BODY=$(echo "$INPUT" | python3 -c "
import sys, json
entities = json.load(sys.stdin)
print(json.dumps({'entities': entities}))
")
else
    BODY="$INPUT"
fi

# ── Validate JSON ─────────────────────────────────────────────────────

ENTITY_COUNT=$(echo "$BODY" | python3 -c "
import sys, json
data = json.load(sys.stdin)
if 'entities' not in data:
    print('Error: JSON must have an \"entities\" key', file=sys.stderr)
    sys.exit(1)
print(len(data['entities']))
")

echo "=== DarshanDB Bulk Import ==="
echo "  Server:   $SERVER_URL"
echo "  Entities: $ENTITY_COUNT"
echo ""

# ── Build auth header ─────────────────────────────────────────────────

AUTH_HEADER=""
if [ -n "$TOKEN" ]; then
    AUTH_HEADER="-H \"Authorization: Bearer $TOKEN\""
fi

# ── Send request ──────────────────────────────────────────────────────

START_TIME=$(python3 -c "import time; print(time.time())")

HTTP_CODE=$(curl -s -o /tmp/darshandb-bulk-response.json -w "%{http_code}" \
    -X POST \
    -H "Content-Type: application/json" \
    ${TOKEN:+-H "Authorization: Bearer $TOKEN"} \
    --data-raw "$BODY" \
    "${SERVER_URL}/api/admin/bulk-load")

END_TIME=$(python3 -c "import time; print(time.time())")

# ── Report results ────────────────────────────────────────────────────

if [ "$HTTP_CODE" -ge 200 ] && [ "$HTTP_CODE" -lt 300 ]; then
    RESPONSE=$(cat /tmp/darshandb-bulk-response.json)
    TRIPLES=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('triples_loaded', '?'))")
    TX_ID=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('tx_id', '?'))")
    DURATION=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('duration_ms', '?'))")
    RATE=$(echo "$RESPONSE" | python3 -c "import sys,json; r=json.load(sys.stdin).get('rate_per_sec', 0); print(f'{r:,.0f}')")

    WALL_TIME=$(python3 -c "print(f'{$END_TIME - $START_TIME:.2f}')")

    echo "=== Import Complete ==="
    echo "  Status:          $HTTP_CODE OK"
    echo "  Triples loaded:  $TRIPLES"
    echo "  Transaction ID:  $TX_ID"
    echo "  Server duration: ${DURATION}ms"
    echo "  Wall-clock time: ${WALL_TIME}s"
    echo "  Throughput:      $RATE triples/sec"
else
    echo "=== Import Failed ===" >&2
    echo "  HTTP status: $HTTP_CODE" >&2
    echo "  Response:" >&2
    cat /tmp/darshandb-bulk-response.json >&2
    echo "" >&2
    exit 1
fi

rm -f /tmp/darshandb-bulk-response.json
