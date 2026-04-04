#!/usr/bin/env bash
# DarshanDB Server Function Invocation via cURL
# Requires: DARSHAN_TOKEN env var (run auth.sh first)

set -euo pipefail

API="${DARSHAN_URL:-http://localhost:7700}/api"
TOKEN="${DARSHAN_TOKEN:?Run auth.sh first to set DARSHAN_TOKEN}"

echo "=== Server Function Examples ==="
echo ""

# --- Call a query function ---
echo "--- Call listTodos query ---"
curl -s -X POST "$API/fn/listTodos" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}' | jq .
echo ""

# --- Call a mutation function ---
echo "--- Call createTodo mutation ---"
curl -s -X POST "$API/fn/createTodo" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title": "Created via server function"}' | jq .
echo ""

# --- Server health ---
echo "--- Server health ---"
curl -s "$API/admin/health" | jq .
echo ""

# --- Schema introspection ---
echo "--- Current schema ---"
curl -s "$API/admin/schema" \
  -H "Authorization: Bearer $TOKEN" | jq .
echo ""

echo "=== Done ==="
