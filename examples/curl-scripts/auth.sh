#!/usr/bin/env bash
# DarshanDB Authentication via cURL
# Usage: source auth.sh

set -euo pipefail

API="${DARSHAN_URL:-http://localhost:7700}/api"
EMAIL="${1:-demo@example.com}"
PASSWORD="${2:-demo1234}"

echo "--- Sign Up ---"
curl -s -X POST "$API/auth/signup" \
  -H "Content-Type: application/json" \
  -d "{\"email\": \"$EMAIL\", \"password\": \"$PASSWORD\"}" | jq .

echo ""
echo "--- Sign In ---"
RESPONSE=$(curl -s -X POST "$API/auth/signin" \
  -H "Content-Type: application/json" \
  -d "{\"email\": \"$EMAIL\", \"password\": \"$PASSWORD\"}")

echo "$RESPONSE" | jq .

# Export token for use by other scripts
export DARSHAN_TOKEN=$(echo "$RESPONSE" | jq -r '.accessToken')
echo ""
echo "Token exported as DARSHAN_TOKEN"

echo ""
echo "--- Get Current User ---"
curl -s "$API/auth/me" \
  -H "Authorization: Bearer $DARSHAN_TOKEN" | jq .
