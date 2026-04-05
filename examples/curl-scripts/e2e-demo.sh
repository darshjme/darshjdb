#!/usr/bin/env bash
# =============================================================================
# DarshanDB End-to-End Demo
# =============================================================================
#
# A guided walkthrough of DarshanDB's REST API. Run this after starting
# the server to see the full create -> read -> update -> query -> delete cycle.
#
# Prerequisites:
#   1. Postgres running (docker compose up postgres -d)
#   2. Server running   (cargo run --bin ddb-server)
#
# Usage:
#   ./examples/curl-scripts/e2e-demo.sh
#
# =============================================================================

set -euo pipefail

API="${DARSHAN_URL:-http://localhost:7700}/api"

echo "============================================="
echo "  DarshanDB REST API — End-to-End Demo"
echo "============================================="
echo ""
echo "Server: $API"
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 1. HEALTH CHECK
# ─────────────────────────────────────────────────────────────────────────────
#
# The /health endpoint is unauthenticated — useful for load balancers
# and uptime monitors.

echo "--- 1. Health Check ---"
echo "> GET /health"
echo ""

curl -s http://localhost:7700/health | jq .

echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 2. SIGN UP
# ─────────────────────────────────────────────────────────────────────────────
#
# Create an account. DarshanDB returns a JWT access token and a refresh
# token. All subsequent data endpoints require the access token.

echo "--- 2. Sign Up ---"
echo "> POST /api/auth/signup"
echo ""

SIGNUP=$(curl -s -X POST "$API/auth/signup" \
  -H "Content-Type: application/json" \
  -d '{
    "email": "demo@darshan.dev",
    "password": "super-secret-123"
  }')

echo "$SIGNUP" | jq .
TOKEN=$(echo "$SIGNUP" | jq -r '.access_token')
echo ""
echo "Token: $TOKEN"
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 3. CREATE ENTITIES
# ─────────────────────────────────────────────────────────────────────────────
#
# DarshanDB uses an Entity-Attribute-Value (triple store) model under the hood,
# but the REST API looks like any familiar REST API. POST to /api/data/<entity>
# to create a record. DarshanDB infers the schema from the data you send.

echo "--- 3. Create a User ---"
echo "> POST /api/data/users"
echo ""

USER=$(curl -s -X POST "$API/data/users" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Darsh Joshi",
    "email": "darsh@darshan.dev",
    "role": "admin"
  }')

echo "$USER" | jq .
USER_ID=$(echo "$USER" | jq -r '.id')
echo ""

echo "--- 4. Create Todos ---"
echo "> POST /api/data/todos (x2)"
echo ""

TODO1=$(curl -s -X POST "$API/data/todos" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"title\": \"Wire triple store end-to-end\",
    \"done\": false,
    \"priority\": 1,
    \"owner_id\": \"$USER_ID\"
  }")

echo "$TODO1" | jq .
TODO1_ID=$(echo "$TODO1" | jq -r '.id')

TODO2=$(curl -s -X POST "$API/data/todos" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"title\": \"Write documentation\",
    \"done\": true,
    \"priority\": 2,
    \"owner_id\": \"$USER_ID\"
  }")

echo "$TODO2" | jq .
TODO2_ID=$(echo "$TODO2" | jq -r '.id')
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 5. READ ENTITIES
# ─────────────────────────────────────────────────────────────────────────────
#
# GET /api/data/<entity>/<id> fetches a single record.
# GET /api/data/<entity>?limit=N lists records with cursor pagination.

echo "--- 5. Read User Back ---"
echo "> GET /api/data/users/$USER_ID"
echo ""

curl -s "$API/data/users/$USER_ID" \
  -H "Authorization: Bearer $TOKEN" | jq .
echo ""

echo "--- 6. List All Todos ---"
echo "> GET /api/data/todos?limit=10"
echo ""

curl -s "$API/data/todos?limit=10" \
  -H "Authorization: Bearer $TOKEN" | jq .
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 7. QUERY API (DarshanQL)
# ─────────────────────────────────────────────────────────────────────────────
#
# The /api/query endpoint accepts DarshanQL — a declarative query language
# inspired by GraphQL and InstantDB's query syntax. You describe the shape
# of the data you want; the engine figures out the SQL.

echo "--- 7. DarshanQL Query ---"
echo "> POST /api/query"
echo ""

curl -s -X POST "$API/query" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "{ todos { id title done priority owner_id } }"
  }' | jq .
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 8. MUTATION API (batch operations)
# ─────────────────────────────────────────────────────────────────────────────
#
# The /api/mutate endpoint accepts a list of mutations that execute in a
# single Postgres transaction. Supported ops: insert, update, delete, upsert.

echo "--- 8. Batch Mutations ---"
echo "> POST /api/mutate"
echo ""

curl -s -X POST "$API/mutate" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "mutations": [
      {
        "op": "insert",
        "entity": "tags",
        "data": { "name": "urgent", "color": "#ef4444" }
      },
      {
        "op": "insert",
        "entity": "tags",
        "data": { "name": "bug", "color": "#f59e0b" }
      }
    ]
  }' | jq .
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 9. UPDATE
# ─────────────────────────────────────────────────────────────────────────────

echo "--- 9. Update a Todo ---"
echo "> PATCH /api/data/todos/$TODO1_ID"
echo ""

curl -s -X PATCH "$API/data/todos/$TODO1_ID" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{ "done": true }' | jq .
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 10. DELETE
# ─────────────────────────────────────────────────────────────────────────────

echo "--- 10. Delete a Todo ---"
echo "> DELETE /api/data/todos/$TODO1_ID"
echo ""

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE "$API/data/todos/$TODO1_ID" \
  -H "Authorization: Bearer $TOKEN")

echo "HTTP $HTTP_CODE (204 = deleted)"
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 11. VERIFY DELETION
# ─────────────────────────────────────────────────────────────────────────────

echo "--- 11. Verify Deletion ---"
echo "> GET /api/data/todos/$TODO1_ID"
echo ""

curl -s "$API/data/todos/$TODO1_ID" \
  -H "Authorization: Bearer $TOKEN" | jq .
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# 12. OPENAPI SPEC
# ─────────────────────────────────────────────────────────────────────────────

echo "--- 12. OpenAPI Spec ---"
echo "> GET /api/openapi.json (first 5 lines)"
echo ""

curl -s "$API/openapi.json" | jq '{openapi, info, paths: (.paths | keys)}' 2>/dev/null || \
  curl -s "$API/openapi.json" | head -5
echo ""

echo "============================================="
echo "  Demo complete."
echo ""
echo "  What you just saw:"
echo "  - Auth:     signup -> get JWT"
echo "  - CRUD:     create, read, list, update, delete"
echo "  - Query:    DarshanQL via /api/query"
echo "  - Mutate:   batch ops via /api/mutate"
echo "  - Schema:   auto-generated OpenAPI spec"
echo "============================================="
