#!/usr/bin/env bash
# DarshanDB CRUD Operations via cURL
# Requires: DARSHAN_TOKEN env var (run auth.sh first)

set -euo pipefail

API="${DARSHAN_URL:-http://localhost:7700}/api"
TOKEN="${DARSHAN_TOKEN:?Run auth.sh first to set DARSHAN_TOKEN}"

echo "=== DarshanDB CRUD Examples ==="
echo ""

# --- Create ---
echo "--- Create a todo ---"
CREATE_RESPONSE=$(curl -s -X POST "$API/data/todos" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "title": "Learn DarshanDB",
    "done": false,
    "priority": 1,
    "createdAt": '$(date +%s000)'
  }')

echo "$CREATE_RESPONSE" | jq .
TODO_ID=$(echo "$CREATE_RESPONSE" | jq -r '.id')
echo "Created todo: $TODO_ID"
echo ""

# --- Read (single) ---
echo "--- Read the todo ---"
curl -s "$API/data/todos/$TODO_ID" \
  -H "Authorization: Bearer $TOKEN" | jq .
echo ""

# --- Update ---
echo "--- Mark as done ---"
curl -s -X PATCH "$API/data/todos/$TODO_ID" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"done": true}' | jq .
echo ""

# --- List (with query) ---
echo "--- List all todos ---"
curl -s -X POST "$API/query" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "todos": {
      "$order": { "createdAt": "desc" },
      "$limit": 10
    }
  }' | jq .
echo ""

# --- Delete ---
echo "--- Delete the todo ---"
curl -s -X DELETE "$API/data/todos/$TODO_ID" \
  -H "Authorization: Bearer $TOKEN" | jq .
echo ""

echo "=== Done ==="
